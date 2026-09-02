# query_roblox_api

Looks up the real Roblox API — a class's members, one member's signature and docs, deprecation and its replacement, or an enum's items — from the type definitions and API docs already cached for the language server. Not registered while `project.memory_only` is true (`src/server/mod.rs:94-145`).

## Parameters
| Param | Type | Default | Required |
| --- | --- | --- | --- |
| `query` | string | none | yes |
| `member_filter` | string | none | no |
| `include_inherited` | bool | `false` | no |
| `include_documentation` | bool | `false` | no |
| `max_members` | usize | `200` | no |

`RobloxApiRequest`, `src/server/requests.rs:340-359`. The handler clamps `max_members` to `min(max_members, tools.max_listing_entries)` (`src/server/mod.rs:684-686`).

## Implementation
- Handler: `src/server/mod.rs:667-690`, calling `RobloxIndex::api()` then `RobloxApi::answer` (`src/roblox/api.rs:228-259`).
- Source of truth is the language-server install cache, not the network: `globalTypes.<security_level>.d.luau` plus the API docs JSON named by `lsp.documentation_url`, both under `acquire::install_root` (`RobloxApi::load`, `src/roblox/api.rs:200-226`). Security level from `lsp.roblox_security_level`; it is echoed on every answer as `security_level`.
- The parsed API is cached once per process in `RobloxIndex` and never invalidated (`src/roblox/index.rs:94-106`) — unlike the sourcemap and require graph, this has no staleness stamp.
- Definitions are parsed line-wise, not with a Luau parser (`parse_definitions`, `src/roblox/api.rs:585-661`): `declare extern type` / `declare class` headers with `extends`, `@`-attributes producing `deprecated` + `deprecated_use`, a `--#METADATA#` line supplying the service and creatable sets.
- Query dispatch order: `Enum.` prefix -> enum answer; exact type name -> class answer; else split on the **last** `:` or `.` and try owner+member -> member answer; otherwise error with up to 8 suggestions (`SUGGESTIONS`, `src/roblox/api.rs:15`).
- Class answer (`src/roblox/api.rs:261-333`): own members, plus ancestors' members when `include_inherited` (shadowed names skipped, each tagged `inherited_from`). `member_filter` is a case-insensitive substring on the member name, applied after inheritance. Counts reported are `returned_count`, `matched_count` (only when the cap cut it), `total_member_count` (only when the filter cut it), plus `truncated`. Also `is_service`, `creatable`, `inherits` (whole ancestry, nearest first), `extends`, `learn_more_link`.
- When the cap will drop members, `rank_for_truncation` stable-sorts own-before-inherited and non-deprecated-before-deprecated first, so a sample is useful rather than a slice of declaration order (`src/roblox/api.rs:574-576`).
- Member answer (`src/roblox/api.rs:335-381`): walks the `extends` chain from the named owner until the member is found; reports `class` (as asked), `declared_by` (only when an ancestor actually declares it), `member_kind` (`property`/`method`/`event`/`callback`), the raw `declaration`, `deprecated`/`deprecated_use`, prose, `parameters` (excluding `self`), `returns`, and `learn_more_link`. A single member always carries its documentation, regardless of `include_documentation`.
- Enum answer (`src/roblox/api.rs:383-445`): the container type is `Enum<Name>_INTERNAL`; items are its properties whose declaration ends in `Enum<Name>`. `Enum.EasingStyle.Linear` filters items by the trailing segment; otherwise `member_filter` applies. Reports `name`, `items`, `item_count`, `matched_count`, `truncated`.
- Deprecation targets are repaired at load: a `deprecated_use` naming a class that does not declare that member is re-resolved against the deprecated member's own class and ancestry, and dropped when nothing matches, so it never names an uncallable target (`repair_deprecation_targets`, `src/roblox/api.rs:671-717`).

## Edge cases
- Missing cache: errors with a hint to run `biskit-mcp doctor` or start outside memory-only mode. A missing or unparsable docs JSON is only a `tracing::warn` — answers then carry signatures with no prose (`read_docs`, `src/roblox/api.rs:553-566`).
- Empty `query` is refused. Lookups are case-sensitive; suggestions are only offered when the base name is at least 3 characters.
- `Enum.` is required for an enum — a bare `EasingStyle` will not match the `_INTERNAL` container and falls through to the not-found path with enum suggestions only inside the `Enum.` branch.
- Because splitting is on the last `:`/`.`, `Enum.EasingStyle` is handled by the prefix branch before member splitting ever runs.
- `include_documentation` on a class listing adds one doc lookup per member and inflates the answer sharply; the whole result still has to fit `tools.max_answer_chars` (default 50000) or the call errors.
- Answers reflect the cached definition version and security level, not the live Roblox API.

## Relevant files
- `src/server/mod.rs`, `src/server/requests.rs`, `src/server/descriptions.rs`
- `src/roblox/api.rs` — parsing, query dispatch, class/member/enum answers, deprecation repair
- `src/roblox/index.rs` — API caching
- `src/lsp/acquire.rs` — install root the cached files live under
- `src/config.rs` — `lsp.roblox_security_level`, `lsp.documentation_url`, `tools.max_listing_entries`, `tools.max_answer_chars`, `project.memory_only`

Siblings: `mem:Tools/resolve_instance_path`, `mem:Tools/get_require_graph`, `mem:Tools/get_module_context`.
