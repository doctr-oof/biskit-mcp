Finds files whose name or project-relative path matches a glob mask.

## Parameters
- `file_mask` (string, required) — filename glob, for example `"*.luau"` or `"init.*"`.
- `relative_path` (string, optional, defaults to `"."`) — directory to search under, relative to the project root.

## Implementation
- Handler: `src/server/mod.rs:313` (`async fn find_file`), request type `FindFileRequest` in `src/server/requests.rs:51`, description text in `src/server/descriptions.rs:30`.
- Calls `FileTools::find_file` at `src/files.rs:137`.
- `Project::resolve` + `ensure_directory` apply first, so the base must exist, be a directory, and stay inside the project root (symlinks resolved).
- Always recursive: it uses the shared `crate::project::walk_builder` with no depth limit — same gitignore / `project.ignored_paths` / `.git` / `.biskit` rules as `mem:Tools/list_dir`.
- `compile_glob` (`src/files.rs:369`) builds a `globset::Glob`. The matcher is tried against the entry's **file name** first, then against the entry's path relative to the project root, so both `"*.luau"` and `"src/**/init.luau"` work.
- Only regular files are considered; directories are skipped.
- Returns a plain JSON array of project-root-relative paths with `/` separators (`normalize_separators`), sorted at the end.

## Edge cases
- The cap is `tools.max_listing_entries` (default 2000) and the walk `break`s the moment that many are collected. There is no `truncated` flag and no `note` on this tool — a full result and a capped one look identical.
- Because the break happens before the sort, a capped result is the first N in walk order, then sorted; it is not the lexicographically first N of the project.
- Naming an ignored directory as `relative_path` searches it anyway (the walk root is not tested against the overrides). An ignored directory *below* the base is still pruned, because `ignored_paths` patterns are anchored at the project root rather than at the base.
- An invalid glob fails with `invalid glob pattern: <mask>: <error>` plus a hint naming `*`, `?`, `[]`, `**`.
- Path errors mirror `list_dir`: `no such directory`, `not a directory`, `path must be relative to the project root`, `path escapes the project root`.
- The JSON array passes through `Biskit::ok` and errors out if it exceeds `tools.max_answer_chars` (default 50000).

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/files.rs`
- `src/project.rs`
- `src/config.rs`
- `src/errors.rs`

Siblings: `mem:Tools/list_dir`, `mem:Tools/search_for_pattern`.
