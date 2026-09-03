# SIFT-006: Repository indexing

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-002, SIFT-003, SIFT-005  
**Implementation plan:** [`../plans/SIFT-006-repository-indexing-plan.md`](../plans/SIFT-006-repository-indexing-plan.md)

## Purpose

A store, a chunker, and an embedder exist but nothing connects them, so no
repository has ever been indexed. The part that decides whether this project is
usable is not the first index but the second: if every commit costs a full
re-embed, the index is either always stale or always rebuilding, and an agent
querying it gets answers about code that no longer exists. This task walks a
repository, produces and persists chunks, and — on every subsequent run — does
work proportional to what git says changed rather than to the size of the
repository.

## Requirements

### Full index

- Indexing a repository from empty produces one chunk per symbol the chunker
  emits, for every non-excluded file in a supported language, with its embedding
  persisted and the store's correspondence check passing at the end.
- Traversal honours the exclusion rules before reading file contents, and the
  counts of files indexed, skipped as excluded, skipped as unsupported, and
  failed to parse are reported.
- Parsing and hashing use the available cores; embedding is batched to the
  configured limit rather than issued per chunk.
- The commit the index was built from is recorded, because incremental update
  needs a starting point and "whatever was there last time" is not one.
- An interrupted index leaves a store that opens and verifies, and re-running
  resumes rather than restarting from nothing.

### Incremental update

- An update determines the changed set from git — added, modified, deleted,
  renamed — between the recorded commit and the current one, and re-parses only
  the files in that set.
- A chunk whose content hash already exists reuses its embedding and is not
  re-embedded, so a file whose formatting changed but whose bodies did not costs
  no GPU time.
- A chunk whose hash is new is embedded and appended; a chunk that no longer
  exists in a re-parsed file has its record removed.
- A pure rename re-embeds nothing, because hashes exclude the path.
- Uncommitted working-tree changes are handled by a stated rule, and whichever
  rule is chosen, a query never returns a symbol that does not exist at the
  indexed revision without that being visible.
- After an update, the recorded commit advances and the correspondence check
  passes.
- Update cost is proportional to the changed set: a one-file commit in a large
  repository does not read, parse, or embed the unchanged files.

### Compaction policy

- Dead positions accumulated by updates are reclaimed when their fraction
  crosses a stated threshold, and the threshold and its rationale are recorded.
- Compaction is triggered by the pipeline, not by the store, and never runs in
  the middle of a query-serving path in a way that makes a query wait on it.

### Observability

- A run reports files processed, chunks added, chunks reused, chunks removed,
  embeddings computed, wall-clock time, and time attributed to parsing versus
  embedding, because "indexing is slow" is not actionable without the split.
- Progress is visible during a long run rather than only at the end.

## Constraints and non-goals

- No searching. Nothing in this task ranks or queries; it only builds what
  SIFT-007 and SIFT-008 read.
- No daemon, no socket, no MCP surface, no background watching of the
  filesystem. Indexing is invoked; SIFT-010 decides when.
- No filesystem-watch-driven continuous reindexing. The trigger is an explicit
  call or a commit-to-commit update, and a watcher is the obvious addition this
  rules out — it introduces a concurrent writer the store does not support.
- No cross-repository index. One repository per store; multiple repositories are
  multiple stores.
- No re-chunking strategy changes, no hashing changes. Those belong to SIFT-003
  and a change there invalidates indexes by design.
- No indexing of git history, prior revisions, or blame data. Only the working
  revision. History mining for evaluation is SIFT-012 and reads git directly.
- No partial-file incremental parsing. A changed file is re-parsed whole; that
  is cheap and the alternative is a correctness risk for no measured gain.

## Acceptance criteria

### Agent-verifiable

1. Indexing a fixture repository produces the expected chunk count and set of
   symbols, and the store's correspondence check passes.
2. Re-indexing with no changes embeds nothing, adds nothing, removes nothing,
   and leaves the store byte-identical apart from the recorded commit.
3. A commit that edits one function's body re-embeds exactly one chunk, verified
   by the reported embedding count, in a fixture repository of many files.
4. A commit that renames a file without changing content re-embeds nothing and
   updates the recorded path for every affected chunk.
5. A commit that deletes a file removes exactly its chunks, and those positions
   are reported dead and absent from enumeration.
6. Reordering functions within a file without changing their bodies re-embeds
   nothing.
7. Crossing the dead-fraction threshold triggers compaction, and the resulting
   store passes the correspondence check with the live count unchanged.
8. An index run interrupted partway leaves a store that opens and verifies, and
   the subsequent run completes the index.
9. Excluded paths are never opened, asserted at the traversal level.
10. The reported counters sum consistently: chunks added plus reused plus
    removed reconcile with the store's live count before and after.
11. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. A real repository of at least 50,000 lines is indexed from empty on the
   target machine, and total wall-clock time, chunk count, store size, and the
   parse-versus-embed split are reported.  
   Command: `cargo run --release -p indexing --example index_repo -- <repo-path> --timing`
2. The most recent ten commits of that repository are replayed as incremental
   updates, and per-commit wall-clock time and embeddings computed are reported,
   confirming cost tracks the diff rather than the repository.  
   Command: `cargo run --release -p indexing --example replay_commits -- <repo-path> --count 10`
3. Peak GPU memory during a full index is measured with a desktop session
   attached and reported against the budget.  
   Command: `cargo run --release -p indexing --example index_repo -- <repo-path> --report-vram`
