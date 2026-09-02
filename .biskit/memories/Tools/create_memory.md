# create_memory

Writes a markdown memory file, refusing an existing name unless `overwrite` is set.

## Parameters
`CreateMemoryRequest` (`src/server/requests.rs:12-21`):
- `memory_name` — string, required. Without `.md`; nest with `/`.
- `content` — string, required. Markdown body. Cross-reference other memories as `mem:name` in backticks.
- `overwrite` — bool, optional, `#[serde(default)]` so it defaults to `false`. Replaces an existing memory wholesale.

## Implementation
- Handler: `src/server/mod.rs:225-242`. Calls `MemoryStore::create(name, content, overwrite)`, then renders `Wrote memory <name>.` or `Replaced memory <name>.` depending on `CreateOutcome::replaced`.
- `MemoryStore::create` (`src/memory.rs:80-100`): resolves the path, records whether it already existed, errors when it exists and `overwrite` is false, calls `Project::bootstrap()`, creates the parent directories, then `std::fs::write`.
- `bootstrap` (`src/project.rs:62-88`) is why creating the first memory in a fresh project also creates `.biskit/`, `.biskit/memories/`, `.biskit/.gitignore` (containing `settings.local.yml`), `.biskit/settings.yml`, and `.biskit/settings.local.yml`. Existing files are left untouched.
- Name normalisation is `stem()` — trim, strip surrounding `/`, drop a trailing `.md` — so `create_memory` with `notes.md` and with `notes` target the same file. Nesting `/` segments become real directories under `.biskit/memories`.
- The content is written byte for byte. No frontmatter, no timestamp, no trailing newline is added.

## Edge cases
- Name taken without `overwrite`: `memory already exists: <name>`, hint `amend it with edit_memory, or pass overwrite: true to replace it wholesale` (`src/memory.rs:84-88`). Prefer `mem:Tools/edit_memory` for amendments; a wholesale rewrite loses detail.
- `overwrite: true` is a full replacement, not a merge, and there is no backup.
- Empty name after normalisation, and any name escaping `.biskit/memories` (`../`, absolute, drive/root prefix, or a symlink pointing out of the root), are refused by `path_for` / `Project::resolve`.
- No validation of `content`: an empty string writes an empty memory, and a broken `mem:` pointer is never checked.
- Reply is a one-line confirmation through `text()`, so the answer limit is irrelevant in practice; the request body itself is not size-checked.
- Nothing enforces a naming convention — `initial_instructions` and the manual are the only place conventions live.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/errors.rs`
- `src/config.rs`

Siblings: `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`, `mem:Tools/read_memory`, `mem:Tools/list_memories`, `mem:Tools/initial_instructions`.
