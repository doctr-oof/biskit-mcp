# delete_memory

Deletes one memory file and prunes the directories it emptied.

## Parameters
`MemoryNameRequest` (`src/server/requests.rs:6-10`) — the same struct `read_memory` takes:
- `memory_name` — string, required. Without `.md`; nest with `/`.

## Implementation
- Handler: `src/server/mod.rs:244-256`. Calls `MemoryStore::delete`, then returns `Deleted memory <name>.`.
- `MemoryStore::delete` (`src/memory.rs:102-111`): resolves the path, errors if it is not a file, `std::fs::remove_file`, then `prune_empty_dirs`.
- `prune_empty_dirs` (`src/memory.rs:286-306`) walks upward from the deleted file's parent, removing each directory that is now empty, and stops at the memories root, at the first non-empty directory, at the first `remove_dir` failure, or as soon as the cursor leaves the memories root. So deleting `nested/deep/leaf` leaves no empty `nested/deep` behind, but a sibling file or a stray non-`.md` file anywhere on that chain stops the pruning there.

## Edge cases
- Missing memory: `memory not found: <name>` with hint `call list_memories to see which memories exist`.
- Deletion is unconditional — no confirmation, no `overwrite`-style guard, and no backup. It is the only destructive memory tool with no safety flag.
- `mem:` pointers in other memories are **not** rewritten or reported. Deleting a referenced memory leaves dangling `mem:` pointers; only `rename_memory` maintains references.
- Name normalisation (`stem()`) and the traversal guard are shared with the other memory tools — `notes`, `notes.md`, and `/notes/` all resolve to the same file; anything escaping `.biskit/memories` is refused.
- The pruning ignores errors quietly; a directory it could not remove just ends the loop rather than failing the call.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/errors.rs`

Siblings: `mem:Tools/create_memory`, `mem:Tools/rename_memory`, `mem:Tools/edit_memory`, `mem:Tools/read_memory`, `mem:Tools/list_memories`, `mem:Tools/initial_instructions`.
