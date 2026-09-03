# SIFT-003: Symbol chunking

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-001  
**Implementation plan:** [`../plans/SIFT-003-symbol-chunking-plan.md`](../plans/SIFT-003-symbol-chunking-plan.md)

## Purpose

What gets embedded decides what can ever be retrieved, and fixed-size windows
cut through function bodies so that neither half is a coherent answer to
anything. The project direction names symbol-aware chunking as the single
largest quality lever and puts it in Phase 1 for that reason. This task turns a
source file into chunks aligned to the constructs a developer would name —
functions, methods, classes, structs, implementation blocks, modules, tests —
each carrying the metadata needed to triage it, and each keyed by a hash of its
normalized body so that later re-indexing can tell "moved" from "changed".

## Requirements

### Chunk boundaries

- A chunk corresponds to a named source construct, and its line range covers
  that construct including its attached documentation comment.
- Nested constructs do not silently vanish: a class and its methods are both
  addressable, and the rule for whether an enclosing construct is emitted
  alongside its children is stated and consistent across languages.
- Code that belongs to no named construct is not dropped in silence; the
  disposition of file-level code is defined and observable.
- A construct too large to embed is split on statement boundaries, and every
  fragment carries the construct's signature so that a fragment retrieved alone
  is still interpretable.
- Splitting never divides a construct at a point that leaves a fragment which
  cannot be attributed back to its parent symbol.

### Metadata

- Each chunk carries the file path, language, symbol name, symbol kind,
  signature, first line of its documentation if it has one, and its start and
  end lines, matching the record the chunk store holds.
- Line numbers are one-based and refer to the file as it exists on disk, so a
  reported range can be opened in an editor without adjustment.
- Symbol names are qualified enough to be distinguished from a same-named symbol
  elsewhere in the same file.

### Content hashing

- A chunk's hash is computed over its normalized body and excludes the file
  path, so that moving or renaming a file leaves the hash unchanged.
- Normalization is defined precisely enough that two developers would compute
  the same hash for the same body, and is stable across runs and platforms.
- A change to a construct's body changes its hash; a change elsewhere in the
  file does not.
- The hashing scheme is versioned, so a future change to normalization is
  detectable rather than silently invalidating an existing index.

### Exclusions

- Files matching the exclusion list in the project direction — credential and
  key material, dependency and build directories, generated files, binary and
  minified files — are never opened for content, not merely filtered after
  reading.
- Binary content and files above a size threshold are excluded even when their
  path looks ordinary.
- The exclusion decision for any given path is explainable: a caller can ask why
  a path was skipped and get the rule that skipped it.
- A repository's own ignore rules are honoured in addition to the built-in list,
  never instead of it.

### Languages and failure

- The set of supported languages is explicit, and a file in an unsupported
  language is skipped as unsupported rather than chunked badly.
- A file that fails to parse yields no chunks and one diagnostic, and does not
  abort the run — one malformed file must not cost a whole repository index.
- Chunking a file is deterministic: the same bytes yield the same chunks in the
  same order.

## Constraints and non-goals

- No embedding, no storage writes, no database. This task produces records; it
  does not persist them. SIFT-006 connects the two.
- No repository traversal, no git interaction, no incremental logic. Chunking
  operates on files it is handed. Walking a tree is SIFT-006's job, and putting
  it here is the obvious temptation to refuse.
- No cross-file resolution: no call graph, no import following, no type
  inference. Those are Phase 3 change intelligence.
- No language-server or SCIP integration. Parsing only.
- No tokenizer-exact size accounting. The oversize threshold is a defensible
  approximation of the model's context limit; making it exact requires the
  tokenizer, which arrives with SIFT-005, and would couple parsing to a model.
- No support for every language a parser exists for. A small explicit set,
  chosen to cover this repository and the repositories it will be evaluated on.

## Acceptance criteria

### Agent-verifiable

1. For each supported language, a fixture file yields chunks whose symbol names,
   kinds, and line ranges match a committed snapshot exactly.
2. Opening a reported line range in the fixture file recovers the construct: the
   first and last lines of the range are asserted to be the construct's own.
3. A construct exceeding the size threshold produces multiple fragments, each
   carrying the parent signature and attributable to the parent symbol.
4. Moving a fixture file to a new path leaves every chunk hash unchanged; editing
   one construct's body changes exactly that construct's hash and no other.
5. Every category in the project direction's exclusion list is covered by a test
   asserting the path is refused, and the test asserts the file's contents were
   never read.
6. A file exceeding the size threshold and a file containing binary content are
   both excluded regardless of extension.
7. A deliberately malformed source file produces zero chunks, one diagnostic, and
   no failure of the surrounding run.
8. Chunking the same input twice produces byte-identical output.
9. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Chunk output for a real repository of at least 50,000 lines is inspected and
   confirmed to align with symbols a developer would name, with the count of
   unsupported and unparsed files reported.  
   Command: `cargo run --release -p indexing --example dump_chunks -- <repo-path>`
2. Chunking throughput over that repository is measured and reported in files and
   chunks per second, establishing the cost floor for a full index.  
   Command: `cargo run --release -p indexing --example dump_chunks -- <repo-path> --timing`
