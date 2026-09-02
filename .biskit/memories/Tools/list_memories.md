# list_memories

Returns the names of every memory in the project store, as a JSON array.

## Parameters
None. The request type is `NoArguments {}` (`src/server/requests.rs:418`).

## Implementation
- Handler: `src/server/mod.rs:202-212`. Delegates to `MemoryStore::list()` and serialises the `Vec<String>` through `Biskit::ok`.
- `MemoryStore::list` (`src/memory.rs:60-69`) returns an empty vector when `.biskit/memories` does not exist, otherwise recurses with `collect` (`src/memory.rs:373-388`) and sorts the names.
- `collect` keeps only files whose extension is exactly `md` (`MEMORY_EXTENSION`, `src/memory.rs:10`), strips the memories root prefix, converts separators with `project::normalize_separators`, and drops the `.md` suffix. So a nested memory comes back as `architecture/rendering`, always forward-slashed regardless of platform.
- Sorting is a plain byte sort of the full relative names, so nested names group by their leading segment.

## Edge cases
- Non-`.md` files inside `.biskit/memories` are invisible to every memory tool, but the directories holding them still block `delete_memory`'s empty-directory pruning.
- Returned via `ok()` (`src/server/mod.rs:60-76`), which *refuses* rather than truncates when the serialised JSON exceeds `tools.max_answer_chars` (default 50000). The error carries `OVERRUN_HINT` from `src/server/results.rs:11`.
- This walk is not the ignore-aware project walker; `project.ignored_paths` and `.gitignore` have no effect here.
- `MemoryStore::list` is also what `get_status` counts for `memories.count` (`src/status.rs:148`) and what `initial_instructions` renders as its index.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/status.rs`
- `src/config.rs`

Siblings: `mem:Tools/initial_instructions`, `mem:Tools/read_memory`, `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`.
