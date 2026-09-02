# read_memory

Returns one memory's full markdown body as text.

## Parameters
`MemoryNameRequest` (`src/server/requests.rs:6-10`):
- `memory_name` — string, required, no default. "Memory name, without the .md extension. Nest with `/`."

## Implementation
- Handler: `src/server/mod.rs:214-223`. Calls `MemoryStore::read(&request.memory_name)` and returns the body through `Biskit::text`.
- `MemoryStore::read` (`src/memory.rs:71-78`) resolves the path with `path_for`, errors if it is not a file, then `std::fs::read_to_string`.
- `path_for` (`src/memory.rs:261-284`) normalises the name with `stem()`: trim whitespace, trim leading and trailing `/`, strip a trailing `.md`. So `read_memory` accepts `notes`, `notes.md`, `/notes/`, and ` notes ` as the same memory. The on-disk path is `<project root>/.biskit/memories/<stem>.md`.
- The content is returned verbatim; no rendering, no `mem:` resolution. `mem:` pointers are plain text that only `rename_memory` ever rewrites.

## Edge cases
- Missing memory: `memory not found: <name>` with hint `call list_memories to see which memories exist` (`UNKNOWN_MEMORY_HINT`, `src/memory.rs:13`).
- Empty name after normalisation: `memory name must not be empty`, hinted toward `"style-guide"` / `"architecture/rendering"`.
- Path traversal: `path_for` goes through `Project::resolve` (which rejects absolute paths, root/drive prefixes, and `..` that escapes the root, including via symlinks) and then re-checks `starts_with(memories_dir)`, erroring with `memory name escapes the memories directory`.
- Returned via `text()`, so a large memory is truncated at `tools.max_answer_chars` (default 50000, `0` disables) at a UTF-8 char boundary (`truncate_at_char_boundary`, `src/server/results.rs:14`) with a `[truncated: ...]` footer. A memory can therefore be read incompletely without an error — check for that footer.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/errors.rs`
- `src/config.rs`

Siblings: `mem:Tools/list_memories`, `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`, `mem:Tools/initial_instructions`.
