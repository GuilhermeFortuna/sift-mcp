# SIFT-003 implementation plan: Symbol chunking

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-003-symbol-chunking-spec.md`](../specs/SIFT-003-symbol-chunking-spec.md)  
**Depends on:** SIFT-001

## Current-system context

SIFT-001 leaves `crates/indexing` empty. SIFT-002 is a sibling task with no
dependency in either direction, and it defines `ChunkRecord` and `ContentHash`
in `crates/storage`. Those types already carry exactly the fields this task must
produce — repository, file, language, symbol, symbol type, signature,
`doc_first_line`, `line_start`, `line_end`, `content_hash` — so this task
produces `storage::ChunkRecord` values rather than inventing a parallel type.
`blake3` is already declared in `[workspace.dependencies]` by SIFT-002.

The exclusion list is written out in `docs/cuda-mcp-rtx2060-plan.md` under
*Index-time exclusions* and is marked non-negotiable; it exists only as prose.
The gap this task closes is that no source file can be turned into chunks, and
the design's single largest quality lever — symbol-aware boundaries — has no
implementation.

## Interfaces produced

```rust
// crates/indexing/src/language.rs
/// The explicit supported set. Anything else is Unsupported, never guessed at.
pub enum Language { Rust, Python, TypeScript, JavaScript, Go, C, Cpp }

impl Language {
    /// By extension only. Content sniffing is not used: a wrong guess produces
    /// silently bad chunks, while an unsupported file produces a counted skip.
    pub fn from_path(path: &Path) -> Option<Language>;
}
```

```rust
// crates/indexing/src/exclusions.rs
/// Why a path was skipped. Returned so a caller can explain any decision.
pub enum SkipReason {
    SecretPattern(&'static str),   // the pattern that matched
    VendorDirectory(&'static str),
    GeneratedPattern(&'static str),
    GitIgnored,
    TooLarge { bytes: u64, limit: u64 },
    BinaryContent,
    UnsupportedLanguage,
}

pub struct Exclusions { /* compiled built-in globs + repository ignore rules */ }

impl Exclusions {
    pub fn for_repository(root: &Path) -> Result<Self, ChunkError>;
    /// Path-only decision. Made before the file is opened.
    pub fn check_path(&self, path: &Path) -> Option<SkipReason>;
    /// Content decision, on the first bytes only, after check_path passes.
    pub fn check_head(&self, head: &[u8]) -> Option<SkipReason>;
}
```

```rust
// crates/indexing/src/chunker.rs
pub struct Chunker { /* one tree-sitter Parser and query set per Language */ }

/// A chunk plus the body text to embed. The record is what SIFT-002 stores;
/// the body is what SIFT-005 embeds and is not persisted.
pub struct Chunk {
    pub record: storage::ChunkRecord,
    pub body: String,
    /// Some(n) when the parent symbol was split; n is 0-based fragment index.
    pub fragment: Option<u32>,
}

/// Everything one file produced, including why nothing was produced.
pub struct FileChunks {
    pub chunks: Vec<Chunk>,
    pub diagnostics: Vec<ChunkDiagnostic>,
}

impl Chunker {
    pub fn new() -> Result<Self, ChunkError>;
    /// `source` is the file's bytes; `rel_path` is repository-relative.
    pub fn chunk_file(&mut self, rel_path: &str, language: Language, source: &str)
        -> FileChunks;
}
```

```rust
// crates/indexing/src/hash.rs
/// Version of the normalization rules. Bumping it invalidates every hash,
/// which is why it is recorded in the store rather than implied.
pub const HASH_SCHEME_VERSION: u32 = 1;

/// blake3 over HASH_SCHEME_VERSION, the language, the symbol name, and the
/// normalized body. Excludes the file path so a move does not re-embed.
pub fn content_hash(language: Language, symbol: &str, body: &str) -> storage::ContentHash;

/// Trailing whitespace stripped per line, line endings normalized to \n,
/// leading and trailing blank lines removed, common leading indentation removed.
/// Interior blank lines and interior indentation are preserved.
pub fn normalize_body(body: &str) -> String;
```

## Implementation decisions

- **Symbol extraction uses one tree-sitter query per language rather than
  hand-written cursor walks.** A query is a declarative list of the node kinds
  that count as symbols, reviewable next to the language's grammar; a cursor
  walk buries the same list in control flow where adding a language means
  rewriting a traversal.

- **A container and its members are both emitted, and the container's own chunk
  covers only its header and any body outside its members.** Emitting the whole
  class as well as each method duplicates every method body in the index,
  inflating the matrix and letting one symbol occupy several top-k slots.
  Emitting only members loses the class-level documentation, which is often the
  best answer to "what is this for".

- **File-level code outside any symbol is emitted as a single synthetic chunk
  per file with symbol type `file_prelude`, only when it exceeds a minimum size
  threshold.** Imports and a licence header are noise that would occupy a row
  per file across the whole corpus; a substantive module body is a real answer.
  The threshold and the synthetic type make the disposition observable rather
  than silent.

- **The line range includes the attached documentation comment, and the
  documentation comment is included in the embedded body.** The doc comment is
  usually the most retrievable text a symbol has; excluding it from the body
  discards the natural-language description that dense retrieval depends on.

- **Symbol names are qualified with their enclosing container, separated by
  `::` regardless of language.** Two `update` methods in one file are otherwise
  indistinguishable in a result list, which is where the agent reads them. A
  single separator across languages keeps `get_symbol` in SIFT-011 from needing
  per-language parsing of its argument.

- **The oversize threshold is a character count approximating the model's
  maximum sequence length, with the conversion factor and its basis recorded as
  a named constant.** The exact answer needs the tokenizer, which arrives in
  SIFT-005; coupling the parser to a model would make chunking depend on the
  GPU crate and break the CPU-only build. A conservative approximation costs a
  few unnecessary splits and nothing else.

- **Splitting happens at statement boundaries taken from the parse tree, not at
  line or character counts, and every fragment is prefixed with the parent
  signature.** A fragment cut mid-expression is not interpretable alone, which
  is exactly the failure the design document calls out.

- **Fragments share the parent's symbol name and carry a `fragment` index rather
  than getting synthesized distinct names.** A synthesized name like
  `Tracker::update#2` appears in results and in `get_symbol` arguments, where it
  does not correspond to anything a developer can look up.

- **`normalize_body` strips trailing whitespace, normalizes line endings, trims
  leading and trailing blank lines, and removes common leading indentation.**
  Line endings and trailing whitespace differ across platforms and editors and
  would make the same code hash differently on two machines. Common indentation
  changes when a function moves into a nested block without its logic changing.
  Interior blank lines and interior indentation are preserved because changing
  them is a real edit.

- **The hash covers `HASH_SCHEME_VERSION`, the language, and the symbol name in
  addition to the normalized body.** Two languages can produce byte-identical
  bodies for different meanings; a renamed function with an unchanged body is a
  different symbol and must re-embed. The version is inside the hash so that a
  future normalization change produces universally different hashes rather than
  a subset that silently collide with old ones.

- **`check_path` runs before the file is opened and `check_head` reads only the
  first few kilobytes.** The spec requires that excluded files are never read
  for content, and a secret file that is opened has already been read into
  memory and possibly into a log.

- **Repository ignore rules are applied in addition to the built-in list, never
  as a substitute.** A repository that does not ignore `node_modules` still must
  not be indexed through it, and a repository that ignores its own build output
  should be respected.

- **A parse failure yields zero chunks and one diagnostic; the tree-sitter error
  node count is used as the signal rather than a boolean.** A file with one
  error node in an otherwise good parse is common in real repositories and its
  good symbols are worth keeping; a file that is mostly error nodes is not.
  The threshold is a named constant with its value stated.

- **Chunk order is the parse tree's source order, and the chunker sorts nothing.**
  Determinism is required and source order is the only order that is stable
  under an unrelated edit elsewhere in the file.

## Ordered implementation

1. Create the branch `SIFT-003-symbol-chunking`.
2. Declare `tree-sitter`, the seven grammar crates, `ignore`, `globset`, and
   `memchr` in `[workspace.dependencies]` and inherit them in
   `crates/indexing`. Add a dependency on `crates/storage` for `ChunkRecord` and
   `ContentHash`. Confirm `./ci.sh` passes. Commit.
3. Write failing unit tests for `normalize_body`: CRLF and LF inputs of the same
   text normalize identically; trailing spaces are removed; leading and trailing
   blank lines are removed; a body indented by four spaces throughout normalizes
   equal to the same body unindented; an interior blank line survives. Run and
   confirm they fail. Implement `normalize_body`. Confirm they pass. Commit.
4. Write failing unit tests for `content_hash`: the same body under two
   languages hashes differently; the same body under two symbol names hashes
   differently; the same body with different indentation hashes identically;
   the hash is stable across two runs in separate processes. Run and confirm
   they fail. Implement `content_hash`. Confirm they pass. Commit.
5. Write failing tests for `Exclusions::check_path`, one per category in the
   design document's exclusion list — `.env`, `.env.local`, `*.pem`, `*.key`,
   `id_rsa`, `credentials.json`, `secrets.yaml`, `node_modules/`, `vendor/`,
   `target/`, `dist/`, `.venv/`, `foo_pb2.py`, `bar.generated.ts` — each
   asserting the returned `SkipReason` names the matching rule. Add a test
   asserting a repository `.gitignore` entry is honoured and that a built-in
   rule still applies to a path the repository does not ignore. Run and confirm
   they fail. Implement `Exclusions`. Confirm they pass. Commit.
6. Write a failing test asserting excluded files are never opened: place a
   sentinel file in an excluded path with permissions that make reading it fail,
   and assert traversal-level checking returns a `SkipReason` without an I/O
   error. Add failing tests for `check_head`: a file of null bytes returns
   `BinaryContent`; a file above the size limit returns `TooLarge` naming both
   the size and the limit, regardless of a `.rs` extension. Run and confirm they
   fail. Implement the content checks. Confirm they pass. Commit.
7. Add a Rust fixture file containing a free function, a struct with an impl
   block and two methods, a documented function, a nested module, and a test
   function. Write a failing snapshot test asserting the exact set of symbol
   names, symbol types, and line ranges. Run and confirm it fails. Implement
   `Chunker` for Rust with a tree-sitter query. Confirm the snapshot matches
   after review. Commit.
8. Write a failing test asserting line-range correctness independently of the
   snapshot: for each chunk, the fixture's `line_start` line contains the
   symbol's own declaration or its documentation comment, and `line_end` is the
   construct's last line. Run and confirm it fails. Fix range computation.
   Confirm it passes. Commit.
9. Repeat steps 7 and 8 for Python, TypeScript, JavaScript, Go, C, and C++, one
   language per commit, each with its own fixture and snapshot.
10. Write failing tests for container handling: the fixture's struct with two
    methods yields three chunks, the container chunk's body excludes both method
    bodies, and both methods carry `::`-qualified names. Run and confirm they
    fail. Implement container-minus-members extraction. Confirm they pass.
    Commit.
11. Write failing tests for the file prelude: a fixture whose module-level code
    exceeds the threshold yields a `file_prelude` chunk; one with only imports
    yields none. Run and confirm they fail. Implement the prelude rule. Confirm
    they pass. Commit.
12. Write failing tests for oversize splitting: a generated function exceeding
    the threshold yields more than one fragment, every fragment's body begins
    with the parent signature, every fragment carries the parent symbol name and
    a distinct `fragment` index, and no fragment boundary falls inside a
    statement. Run and confirm they fail. Implement splitting on statement
    boundaries. Confirm they pass. Commit.
13. Write failing tests for move-versus-edit: chunking a fixture, then chunking
    the identical source under a different `rel_path`, yields identical hashes
    for every chunk; editing one function's body changes exactly that chunk's
    hash and leaves every other unchanged. Run and confirm they fail. Confirm
    the hash inputs exclude the path. Confirm they pass. Commit.
14. Write failing tests for malformed input and determinism: a deliberately
    broken fixture yields zero chunks and exactly one diagnostic and does not
    panic; a file with a single error node in an otherwise valid parse still
    yields its valid symbols; chunking the same input twice yields byte-identical
    output. Run and confirm they fail. Implement the error-node threshold.
    Confirm they pass. Commit.
15. Add the `dump_chunks` example: walks a repository applying exclusions,
    chunks every supported file, and prints chunks as JSON, with `--timing`
    reporting files and chunks per second and counts of skipped, unsupported,
    and unparsed files. Commit.
16. Human step: run `cargo run --release -p indexing --example dump_chunks --
    <repo-path>` on a real repository of at least 50,000 lines, read a sample of
    the output, and judge whether chunk boundaries align with symbols a
    developer would name; report the counts of unsupported and unparsed files.
17. Human step: run the same example with `--timing` and report files per
    second, chunks per second, and total chunks.
18. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `normalize_body` across line endings, whitespace, and indentation;
  `content_hash` across language, symbol, indentation, and process boundaries;
  one exclusion test per category in the design document's list; `check_head`
  for binary and oversize content.
- **Integration:** per-language snapshot of symbol names, types, and line
  ranges; container-minus-members extraction; oversize splitting; move-versus-
  edit hash stability; malformed-file handling.
- **Regression:** the committed per-language snapshots are the locked reference;
  any change to boundaries or naming must show as a snapshot diff and be
  justified, since it invalidates every existing index.
- **Manual:** reading chunk output for a real repository; correct means
  boundaries fall on constructs a developer would name and the skipped and
  unparsed counts are explainable.
- **Measurement:** files and chunks per second over a repository of at least
  50,000 lines, three runs, reporting individual values and the median.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p indexing --example dump_chunks -- <repo-path>
cargo run --release -p indexing --example dump_chunks -- <repo-path> --timing
```

## Handoff

Report the seven languages implemented and, for each, the fixture's chunk count
and the symbol types extracted; the value chosen for the oversize character
threshold and the token-conversion basis behind it; the value of the file-prelude
size threshold and the error-node threshold; confirmation that every exclusion
category from the design document has a test and that the never-opened assertion
holds; the evidence that a file move changes no hash and a body edit changes
exactly one; and, from the real repository, total files, files chunked, files
skipped as excluded, files skipped as unsupported, files that failed to parse,
total chunks, and files and chunks per second with individual values and the
median over three runs.
