Lists the inferred types luau-lsp would draw inline over a line range — the cheapest way to see what a function's variables, arguments, and returns resolve to without reading the body. Disabled while `project.memory_only` is true (`LANGUAGE_SERVER_TOOLS`, `src/server/mod.rs:96`).

## Parameters
`InlayHintsRequest`, `src/server/requests.rs:291`.

| name | type | default | required |
| --- | --- | --- | --- |
| `relative_path` | string | — | yes |
| `start_line` | u32, 1-based inclusive | file start | no |
| `end_line` | u32, 1-based inclusive | file end | no |

There is no max-hints parameter: the cap is `tools.max_listing_entries` (default 2000, `src/config.rs:255`), passed in by the handler.

## Implementation
Handler `src/server/mod.rs:542` → `SymbolQuery::inlay_hints` (`src/lsp/queries/inlay.rs:43`), with `self.inner.settings.tools.max_listing_entries` as `max_hints`.

1. Resolve path, `ensure_luau_file`, `handle.session()`, `session.ensure_open` (no disk sweep — that is diagnostics-only, see `mem:Tools/get_file_diagnostics`).
2. `check_line_range` validates both bounds against the file's real length.
3. Empty file short-circuits: `start_line`/`end_line` omitted, `hints: []`, `note: "<path> is empty"`.
4. Builds an LSP `Range` from line `start_line-1` to line `end_line-1`, with the end character set to the UTF-16 width of the last line (`byte_to_utf16_column`), so the range covers whole lines.
5. `session.inlay_hints` → `textDocument/inlayHint`; a null response becomes an empty vec.
6. Results are filtered again to `from..=to` (the server may return hints outside the requested range), sorted by position, then truncated to `max_hints`.

Each entry is `{ line (1-based), column (1-based), label, kind? }`. `label` is `hint.label.into_text().trim()`, e.g. `: number`; `kind` comes from `inlay_hint_kind_label` and is omitted when absent. `truncated: true` is set when more hints were in range than the cap. Fields are skipped when false/None, so their absence means "nothing was cut".

## Edge cases
- An empty hint list gets `note = NO_HINTS_NOTE` (`src/lsp/queries/inlay.rs:11`) explaining that luau-lsp emits a hint only where a type or argument name is not already written out, and that the `luau-lsp.inlayHints.*` keys under `lsp.server_settings` control which kinds are emitted. An empty list alone would read as a failure.
- If the luau-lsp build does not implement `textDocument/inlayHint`, `client::is_unsupported` converts the error into a hint pointing at `find_symbol` with `include_detail` and at `explain_symbol`.
- Line bounds are checked, not clamped: 0, past EOF, or `start_line > end_line` are errors.
- Non-Luau paths refused by `ensure_luau_file`.
- Columns reported are 1-based UTF-16 code units, the same convention the other position tools consume — `COLUMN_HINT` in `src/lsp/queries/mod.rs:43` explicitly points callers here to discover a line's columns of interest.
- Answer capped by `tools.max_answer_chars`; a wide range over a large file can hit the cap before hitting `max_listing_entries`.

Siblings: `mem:Tools/get_file_diagnostics`, `mem:Tools/get_symbol_diagnostics`, `mem:Tools/get_type_definition`, `mem:Tools/get_signature_help`.

## Relevant files
- `src/server/mod.rs` — handler, memory-only gating, `max_listing_entries` wiring
- `src/server/requests.rs` — `InlayHintsRequest`
- `src/server/descriptions.rs` — description arm
- `src/lsp/queries/inlay.rs` — `inlay_hints`, `InlayHintEntry`, `InlayHintResult`, `NO_HINTS_NOTE`
- `src/lsp/queries/mod.rs` — `SymbolQuery`, `check_line_range`, `COLUMN_HINT`
- `src/lsp/session/mod.rs` — `inlay_hints`, `ensure_open`
- `src/lsp/session/handle.rs` — `session`
- `src/lsp/protocol.rs` — `InlayHint`, `Range`, `Position`, `inlay_hint_kind_label`
- `src/lsp/client.rs` — `is_unsupported`
- `src/lines.rs` — `LineIndex`, `byte_to_utf16_column`
- `src/config.rs` — `tools.max_listing_entries`, `tools.max_answer_chars`, `lsp.server_settings`, `project.memory_only`
