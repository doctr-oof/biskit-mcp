Lists the files and directories under a project-relative path.

## Parameters
- `relative_path` (string, required) — directory relative to the project root; `"."` is the root.
- `recursive` (bool, required, no serde default) — descend into subdirectories.

## Implementation
- Handler: `src/server/mod.rs:300` (`async fn list_dir`), request type `ListDirRequest` in `src/server/requests.rs:43`, description text in `src/server/descriptions.rs:25`.
- Calls `FileTools::list_dir` at `src/files.rs:103`.
- `Project::resolve` (`src/project.rs:91`) rejects absolute paths and any path that escapes the root, including through a symlink (`physically_inside`). `ensure_directory` (`src/files.rs:356`) then requires a directory.
- Walks with `crate::project::walk_builder` (`src/project.rs:148`): `hidden(false)` so dotfiles are listed, `git_ignore`/`git_exclude` follow `project.respect_gitignore` (default true), `git_global(false)`, `require_git(false)`, `follow_links(false)`. `.git` and `.biskit` are always filtered out by name at any depth.
- `project.ignored_paths` become negated `Override` patterns anchored at the project **root** (`build_overrides`, `src/project.rs:173`), so a pattern still applies when the walk starts in a subdirectory.
- Non-recursive sets `max_depth(Some(1))`. The base entry itself is skipped; every other entry is named relative to the listed directory (`relativize_to`), with `/` separators on all platforms.
- Result `DirectoryListing` (`src/files.rs:29`): `base` (the listed dir, `"."` for the root), `directories`, `files`, and `truncated` (omitted when false).

## Edge cases
- Both lists are sorted before truncation, so a cut listing is the first entries by name and is stable across calls.
- `truncate_listing` (`src/files.rs:336`) applies `tools.max_listing_entries` (default 2000) to `directories.len() + files.len()`; directories get at most half the budget so they cannot starve files out. Truncation is silent apart from the `truncated` flag — there is no `note` field here.
- Naming an ignored directory as `relative_path` lists it anyway: the walk root is not itself tested against the overrides, so its contents come back.
- Errors: `not a directory: <path>` when the path is a file (hint: pass its parent or use `search_for_pattern`); `no such directory: <path>` when missing; `path must be relative to the project root` for absolute paths; `path escapes the project root` for `..` traversal or an out-of-root symlink. A malformed `project.ignored_paths` entry fails the call with an `invalid project.ignored_paths entry` error rather than being dropped.
- The serialized result still passes through `Biskit::ok` (`src/server/mod.rs:60`), which errors — it does not truncate — when the JSON exceeds `tools.max_answer_chars` (default 50000).

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/files.rs`
- `src/project.rs`
- `src/config.rs`
- `src/errors.rs`

Siblings: `mem:Tools/find_file`, `mem:Tools/search_for_pattern`.
