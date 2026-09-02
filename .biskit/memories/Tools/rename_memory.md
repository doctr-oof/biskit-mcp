# rename_memory

Moves a memory to a new name and rewrites every `mem:` pointer to it across the store.

## Parameters
`RenameMemoryRequest` (`src/server/requests.rs:36-40`):
- `old_name` — string, required.
- `new_name` — string, required. Nesting with `/` moves the memory into a subdirectory.

Neither has a doc comment or default; both are plain required strings.

## Implementation
- Handler: `src/server/mod.rs:277-296`. Calls `MemoryStore::rename`, then returns JSON via `ok()`: `{"from": ..., "to": ..., "updated_references": [{"memory": ..., "occurrences": N}, ...]}`. This is the only memory tool that returns structured JSON rather than prose.
- `MemoryStore::rename` (`src/memory.rs:165-196`): source must be an existing file; target must not `exists()` (a directory at that path counts, so a name that is already a nesting prefix is refused); creates the target's parent directories; `std::fs::rename`; `prune_empty_dirs` on the old location; then rewrites references.
- Reference rewriting is plan-then-write. `plan_reference_rewrites` (`src/memory.rs:222-259`) reads *every* memory first so a read failure surfaces before any write. `rewrite_references` (`src/memory.rs:198-219`) writes each planned file and, on the first write error, restores the ones already written from their in-memory originals and returns the error. If reference rewriting fails at all, `restore_moved_file` (`src/memory.rs:310-315`) moves the memory back to its old path, so a failed rename leaves no dangling pointers.
- The pointer pattern is `mem:([A-Za-z0-9._\-/]*[A-Za-z0-9_\-])` (`src/memory.rs:11`). The trailing character class means punctuation after a pointer is not consumed: `mem:old-name.`, `mem:old-name/`, and `(mem:old-name)` all rewrite while keeping their surrounding punctuation.
- A candidate matches when `stem(reference) == stem(old_name)` (`reference_matches`, `src/memory.rs:317-319`), so `mem:old-name.md` also matches and is rewritten to the bare `mem:new-name` form — the `.md` suffix is dropped as a side effect.
- `updated_references` lists only memories with at least one occurrence, sorted by memory name; the renamed memory's own body is included in the sweep like any other.

## Edge cases
- Missing source: `memory not found: <old_name>` with the `list_memories` hint.
- Occupied target: `memory already exists: <new_name>`, hint `pick a different new_name, or delete the existing memory`. There is no `overwrite` flag here.
- Both names go through the same `stem()` normalisation and traversal guard as the other memory tools, so `.md` suffixes and stray slashes are tolerated and anything escaping `.biskit/memories` is refused.
- Pointers are matched on the whole normalised stem, not on path segments: renaming `Combat` does **not** rewrite `mem:Combat/HitDetection`. Moving a subtree means renaming each child.
- `mem:` pointers in source files, `CLAUDE.md`, or anything outside `.biskit/memories` are never touched — only memories are swept.
- Result goes through `ok()`, so a rename touching a very large number of memories can exceed `tools.max_answer_chars` (default 50000) and be refused *after* the rename and rewrites have already happened on disk.
- The rollback of already-written reference files is best-effort (`let _ = std::fs::write`), as is `restore_moved_file`.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/errors.rs`
- `src/config.rs`

Siblings: `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/delete_memory`, `mem:Tools/read_memory`, `mem:Tools/list_memories`, `mem:Tools/initial_instructions`.
