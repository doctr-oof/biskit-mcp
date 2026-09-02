# get_module_context

Summarizes one Luau module in a single call: instance path, class, run context, direct dependencies and dependents, its public surface, and diagnostic counts. Not registered while `project.memory_only` is true (`src/server/mod.rs:94-145`).

## Parameters
| Param | Type | Default | Required |
| --- | --- | --- | --- |
| `relative_path` | string | none | yes |

`ModuleContextRequest`, `src/server/requests.rs:334-338`. No other knobs: depth is fixed at 1 in both directions and every list is capped by `tools.max_listing_entries` (default 2000), passed in from the handler.

## Implementation
- Handler: `src/server/mod.rs:648-663`, calling `roblox::context::module_context` (`src/roblox/context.rs:68-162`) with the index, the language-server handle, the path, and `tools.max_listing_entries`.
- Validates the path resolves inside the project, is a Luau file (`ensure_luau_file`), and exists; a missing file errors with a hint to use `find_file`/`list_dir`.
- Sourcemap lookup gives `instance_path`, `class_name`, and `service` (top-level ancestor). It takes the **first** node for the file when several exist.
- `role` is derived by `role_of` (`src/roblox/context.rs:178-198`): class wins first — `Script` -> `server`, `LocalScript` -> `client` — then the service name against fixed lists: server (`ServerScriptService`, `ServerStorage`, `RobloxServerStorage`, `ServerPackages`), client (`StarterPlayer`, `StarterGui`, `StarterPack`, `StarterCharacterScripts`), shared (`ReplicatedStorage`, `ReplicatedFirst`, `Workspace`, `Lighting`, `SoundService`, `TextChatService`). Anything else is `unknown`.
- `requires` / `required_by` are one-hop walks of the require graph in each direction; `unresolved_requires` are this module's own unresolved requires. Full resolution semantics are in `mem:Tools/get_require_graph`.
- `api` comes from `SymbolQuery::module_api` (`src/lsp/queries/module_api.rs:92-214`), so it needs the language server. It locates return statements outside function bodies in the comment-blanked source, classifies the returned expression, and lists the returned table's top-level members from the document symbol tree, filling each export's `detail` with one hover request per export. `return_kind` is one of `table`, `table_literal`, `function`, `expression`, `conditional`, `unknown`, `none`. `types` lists `export type` declarations as written.
- `diagnostics` is a count per severity label, from the LSP session for the file (`count_by_severity`, `src/roblox/context.rs:164-173`).
- `sourcemap` is the same `SourcemapReference` with the `stale` flag as `mem:Tools/resolve_instance_path`, judged against `graph.stamp().newest()` rather than a separate walk.

## Edge cases
- The tool degrades instead of failing: every partial answer is explained in `notes`.
  - File not in the sourcemap: note warning it is not synced into the game by the rojo project (unless the sourcemap is stale), and `instance_path`/`class_name`/`service` are absent.
  - File outside the scanned set: note naming `project.respect_gitignore` and `project.ignored_paths`, with empty requires/required_by.
  - Public surface unreadable: note "the public surface could not be read: ..." and `api` omitted.
  - Diagnostics unreadable (no session, or the query failed): note, and `diagnostics` is an empty map.
- `module_api` notes cover its own gaps: no return statement in a `ModuleScript`, a `Script`/`LocalScript` with no surface, a table literal returned inline, a returned function, an empty table, multiple conditional returns, and `MAX_EXPORT_HOVERS` = 50 detail-fill cap (`src/lsp/queries/module_api.rs:11-48, 159-203`).
- Rojo sourcemaps carry no RunContext, so a `Script` set to a client RunContext is still reported as `server`.
- Reaching the API and diagnostics means the module context call can be slow: one hover per export plus a diagnostics settle.
- Answer size is capped by `tools.max_answer_chars`; over it, the call errors rather than truncating.

## Relevant files
- `src/server/mod.rs`, `src/server/requests.rs`, `src/server/descriptions.rs`
- `src/roblox/context.rs` — `ModuleContext`, role inference, notes
- `src/lsp/queries/module_api.rs` — public surface extraction
- `src/roblox/index.rs`, `src/roblox/sourcemap.rs`, `src/roblox/requires/*` — sourcemap and graph inputs
- `src/lsp/session/*`, `src/lsp/protocol.rs` — session and diagnostic severities
- `src/config.rs` — `tools.max_listing_entries`, `tools.max_answer_chars`, `project.memory_only`

Siblings: `mem:Tools/resolve_instance_path`, `mem:Tools/get_require_graph`, `mem:Tools/query_roblox_api`.
