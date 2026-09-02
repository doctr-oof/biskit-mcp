Reports a call's parameter list and which argument a position sits on, without reading the callee. Disabled while `project.memory_only` is true (`LANGUAGE_SERVER_TOOLS`, `src/server/mod.rs:96`).

## Parameters
`SignatureHelpRequest`, `src/server/requests.rs:253`.

| name | type | default | required |
| --- | --- | --- | --- |
| `relative_path` | string | — | yes |
| `line` | u32, 1-based | — | yes |
| `column` | u32, 1-based UTF-16 code units | 1 | no |
| `include_documentation` | bool | false | no |

This is the only position tool with **no** `name_path`: a declaration is never inside a call's parentheses. The handler uses `SymbolPoint::at` (`src/lsp/queries/mod.rs:91`) rather than `SymbolPoint::parse`; it rejects `line: 0` with `LINE_HINT` and normalizes `column: 0` up to 1.

## Implementation
Handler `src/server/mod.rs:563` → `SymbolQuery::signature_help` (`src/lsp/queries/hover.rs:210`).

1. `handle.session()`, then `locate_point` (`src/lsp/queries/mod.rs:192`): resolves the path, `ensure_luau_file`, `ensure_open`, validates the line against the file length and the column against the line's UTF-16 width. No disk sweep (diagnostics-only, see `mem:Tools/get_file_diagnostics`).
2. `session.signature_help` → `textDocument/signatureHelp`. A `null` response is replaced by an empty `SignatureHelp` rather than an error.
3. Each signature is mapped to a `SignatureEntry`: `label` (passed through `strip_unbound_generics`, which removes generic parameters that appear nowhere else in the label), `parameters` (each parameter's `label` resolved against the signature label — the client declares `labelOffsetSupport`, so labels may arrive as offsets), `active_parameter` (the signature's own, falling back to the top-level `active_parameter`), and `documentation` when asked for.
4. Result carries `relative_path`, 1-based `line`/`column`, `signatures`, `active_signature`, and `note`.

Documentation is only populated when `include_documentation` is true, is dropped when empty, and is capped at `MAX_DOCUMENTATION_CHARS = 4000` chars by `cap_documentation`, which appends `\n\n[documentation truncated]`. The cap is applied to signature documentation; parameter documentation is passed through uncapped.

## Edge cases
- Empty `signatures` sets `note = NO_SIGNATURES_NOTE` (`src/lsp/queries/hover.rs:17`): the server does not say why. Usual cause is a position outside the call's parentheses; luau-lsp also answers with nothing at some positions genuinely inside a call, notably the receiver of a `self:` method call.
- If the build does not implement `textDocument/signatureHelp`, `client::is_unsupported` converts the error into a hint pointing at `explain_symbol`, whose resolved type carries the same parameter list.
- The label is the language server's own rendering of the call, so for a variadic callee such as `print` the parameter names come from the call site, not the callee.
- Line past EOF or column past the line's UTF-16 width are hard errors with `LINE_HINT` / `COLUMN_HINT`; nothing is clamped except `column: 0` → 1.
- Answer capped by `tools.max_answer_chars`.

Siblings: `mem:Tools/get_file_diagnostics`, `mem:Tools/get_symbol_diagnostics`, `mem:Tools/get_type_definition`, `mem:Tools/get_inlay_hints`.

## Relevant files
- `src/server/mod.rs` — handler, memory-only gating
- `src/server/requests.rs` — `SignatureHelpRequest`
- `src/server/descriptions.rs` — description arm
- `src/lsp/queries/hover.rs` — `signature_help`, `SignatureEntry`, `SignatureParameter`, `SignatureHelpResult`, `NO_SIGNATURES_NOTE`, `cap_documentation`, `strip_unbound_generics`
- `src/lsp/queries/mod.rs` — `SymbolPoint::at`, `locate_point`, `check_line_bound`, `LINE_HINT`, `COLUMN_HINT`
- `src/lsp/session/mod.rs` — `signature_help`, `ensure_open`, client capability declaration for `signatureHelp`
- `src/lsp/session/handle.rs` — `session`
- `src/lsp/protocol.rs` — `SignatureHelp`, `Documentation`, parameter label resolution
- `src/lsp/client.rs` — `is_unsupported`
- `src/lines.rs` — `LineIndex`, `byte_to_utf16_column`
- `src/config.rs` — `tools.max_answer_chars`, `project.memory_only`
