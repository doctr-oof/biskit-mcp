# resolve_instance_path

Maps between the Roblox DataModel and files on disk, in either direction, using the rojo sourcemap. Not registered while `project.memory_only` is true (`src/server/mod.rs:132-145`, name listed in `LANGUAGE_SERVER_TOOLS` at `src/server/mod.rs:94-110`).

## Parameters
| Param | Type | Default | Required |
| --- | --- | --- | --- |
| `instance_path` | string | none | one of the two |
| `relative_path` | string | none | one of the two |

Defined in `ResolveInstancePathRequest`, `src/server/requests.rs:302-312`. Passing both is an error; passing neither is an error (`Sourcemap::resolve`, `src/roblox/sourcemap.rs:270-306`).

## Implementation
- Handler: `src/server/mod.rs:582-603`. Loads `RobloxIndex::sourcemap()`, then `RobloxIndex::newest_source()`, then calls `Sourcemap::resolve`.
- Sourcemap file comes from `lsp.sourcemap` (default `sourcemap.json`, `src/config.rs:227`). Load errors carry hints: `lsp.sourcemap` unset, or file missing (`src/roblox/sourcemap.rs:96-119`).
- The sourcemap is cached in `RobloxIndex` and reloaded only when the file's (mtime, len) stamp moves (`src/roblox/index.rs:38-57`).
- `instance_path` -> instances: `resolve_instance_path` walks the arena from the root. The leading segment is dropped when it is `game` or the sourcemap root's own name, so `game.X`, `MyGame.X` and `X` all resolve (`src/roblox/sourcemap.rs:325-378`).
- Path parsing (`parse_instance_path`, `src/roblox/sourcemap.rs:389-425`) accepts `.`, `/`, whitespace as separators; bracket string literals (`Shared["Combat"]`); and `:GetService("X")`, `:WaitForChild("X")`, `:FindFirstChild("X")` only — every other method call is refused rather than guessed. Non-literal arguments (`Shared[key]`, `FindFirstChild(name)`) are refused.
- `relative_path` -> instances: `nodes_for_file` normalizes backslashes and leading `./` (`normalize`, `src/roblox/sourcemap.rs:525-532`), then looks up the by-file index. A miss on an `init.luau`/`init.server.luau`/`init.client.luau` etc. falls back to the owning directory's node, because rojo folds init files into their folder (`src/roblox/sourcemap.rs:309-322`, `is_init_file` at `:542`). One file may map to several instances, so `instances` is a list.
- Each result is an `InstanceAnswer`: `instance_path`, `class_name`, `file_paths`, `script_file` (first `.luau`/`.lua` among them), `service` (top-level ancestor under root), `children` count (`describe`, `src/roblox/sourcemap.rs:257-267`).
- Every answer carries `sourcemap: SourcemapReference` — sourcemap path, `modified_epoch_seconds`, `age_seconds`, `instances` count, and `stale` (`src/roblox/sourcemap.rs:179-199`). `stale` is true when the newest Luau file in the scanned set is newer than the sourcemap; it is omitted when either time cannot be read, rather than guessed.
- "Newest Luau source" is computed over the same file set the require graph uses: the project walk plus every Luau file the sourcemap names (`newest_luau_source`/`luau_files`, `src/roblox/requires/build.rs:57-108`). Metadata only, but it is a full project walk plus one stat per file on every call.

## Edge cases
- Instance path resolution is case-sensitive. A missing segment error names up to 24 of the parent's children as suggestions (`SUGGESTED_CHILDREN`, `src/roblox/sourcemap.rs:19`, `:351-375`).
- A file outside the rojo project, or added since the sourcemap was generated, produces "no instance in the sourcemap is built from ..." — the tool never falls back to a filesystem guess.
- The DataModel root is always reported as `game`, whatever the project is named (`root_label`, `src/roblox/sourcemap.rs:381-386`).
- Result size is capped by `tools.max_answer_chars` (default 50000); over it, the call errors rather than truncating, since it goes through `Biskit::ok` (`src/server/mod.rs:60-75`).

## Relevant files
- `src/server/mod.rs` — handler, memory-only gating
- `src/server/requests.rs` — `ResolveInstancePathRequest`
- `src/server/descriptions.rs` — tool description
- `src/roblox/sourcemap.rs` — parsing, indexing, resolution, staleness
- `src/roblox/index.rs` — sourcemap caching, `newest_source`
- `src/roblox/requires/build.rs` — the file set staleness is judged against
- `src/config.rs` — `lsp.sourcemap`, `tools.max_answer_chars`, `project.memory_only`

Siblings: `mem:Tools/get_require_graph`, `mem:Tools/get_module_context`, `mem:Tools/query_roblox_api`.
