Finds where the *type* of a position is declared, usually an `export type` in another module, via `textDocument/typeDefinition`. Disabled while `project.memory_only` is true (`LANGUAGE_SERVER_TOOLS`, `src/server/mod.rs:96`).

## Parameters
`TypeDefinitionRequest`, `src/server/requests.rs:268`.

| name | type | default | required |
| --- | --- | --- | --- |
| `name_path` | string | — | no, mutually exclusive with `line` |
| `relative_path` | string | — | yes |
| `line` | u32, 1-based | — | no, mutually exclusive with `name_path` |
| `column` | u32, 1-based UTF-16 code units | 1 | no |
| `include_body` | bool | false | no |
| `include_detail` | bool | false | no |

`SymbolPoint::parse` (`src/lsp/queries/mod.rs:64`) enforces the choice: passing both errors, passing neither errors, `line: 0` errors, a blank/whitespace `name_path` is treated as absent. Point `line`/`column` at the type's own name — in `local config: PlayerConfig`, at `PlayerConfig`, not `config`.

## Implementation
Handler `src/server/mod.rs:518` → `SymbolQuery::type_definition` (`src/lsp/queries/references.rs:65`).

1. `handle.session()`. Note: no `sync_disk_changes` sweep here — that is a diagnostics-only step (see `mem:Tools/get_file_diagnostics`).
2. `locate_point` (`src/lsp/queries/mod.rs:192`). Name-path form goes through `locate_one` (case-sensitive tree match, errors on zero or ambiguous matches). Line/column form validates the line against `LineIndex` and validates the column against the line's UTF-16 width, then records `SymbolNode::innermost_at` as `resolved.symbol`.
3. `session.type_definition(path, position)` — `textDocument/typeDefinition`, response normalized through `GotoResponse::into_locations`.
4. Non-empty locations → `render_locations` (`src/lsp/queries/references.rs:259`): groups by file, relativizes, resolves each location's innermost symbol for `name_path`/`kind`, and returns `SymbolsByFile` (`{ relative_path: [SymbolMatch] }`).
5. Empty locations → fallback: if the resolved point *is itself* a type declaration (`declares_a_type`, `src/lsp/queries/references.rs:326` — the line starts with optional `export`, then `type`, then the symbol's name), the symbol is rendered directly and returned; otherwise the call errors with `TYPE_DEFINITION_HINT`.

`include_body` attaches a snippet around the location with `DECLARATION_CONTEXT_LINES = 1` line of context. `include_detail` fires a hover per match through `attach_details`, capped by `MAX_DETAIL_HOVERS = 200` (`src/lsp/queries/mod.rs:53`) shared across the whole call; signatures are passed through `strip_unbound_generics`, which drops generic parameters that appear nowhere else in the signature.

## Edge cases
- If the server does not implement `textDocument/typeDefinition`, `client::is_unsupported` turns the error into a hint pointing at `find_declaration`.
- If every location resolves outside the project root, `render_locations` errors naming those paths rather than returning an empty map.
- A value with no written annotation often has no type declaration to find; the error hint steers to `explain_symbol` for the inferred type.
- Column is 1-based and counts UTF-16 code units; a column past the line's width is refused with `COLUMN_HINT`.
- `document_symbols` failures inside `render_locations` degrade to empty symbols with `kind: "Unknown"` rather than erroring.
- Answer capped by `tools.max_answer_chars`.

Siblings: `mem:Tools/get_file_diagnostics`, `mem:Tools/get_symbol_diagnostics`, `mem:Tools/get_inlay_hints`, `mem:Tools/get_signature_help`.

## Relevant files
- `src/server/mod.rs` — handler, memory-only gating
- `src/server/requests.rs` — `TypeDefinitionRequest`
- `src/server/descriptions.rs` — description arm
- `src/lsp/queries/references.rs` — `type_definition`, `render_locations`, `declares_a_type`, `TYPE_DEFINITION_HINT`
- `src/lsp/queries/mod.rs` — `SymbolPoint`, `locate_point`, `locate_one`, `attach_details`, `MAX_DETAIL_HOVERS`
- `src/lsp/queries/render.rs` — `render`, `snippet_around`, `group_locations_by_file`, `RenderOptions`
- `src/lsp/queries/hover.rs` — `split_hover`, `strip_unbound_generics`
- `src/lsp/queries/symbols.rs` — `SymbolMatch`, `SymbolsByFile`
- `src/lsp/session/mod.rs` — `type_definition`, `ensure_open`
- `src/lsp/session/handle.rs` — `session`, `document_symbols`
- `src/lsp/client.rs` — `is_unsupported`
- `src/lines.rs` — `LineIndex`, `byte_to_utf16_column`
- `src/config.rs` — `tools.max_answer_chars`, `project.memory_only`
