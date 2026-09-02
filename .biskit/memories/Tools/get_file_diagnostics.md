Reports one Luau file's diagnostics, optionally clipped to a line range, grouped by severity and by the symbol each diagnostic falls inside. Disabled while `project.memory_only` is true: `LANGUAGE_SERVER_TOOLS` in `src/server/mod.rs:96` drops its route at construction, and `LanguageServerHandle::session` (`src/lsp/session/handle.rs:92`) refuses with a memory-only error even if reached.

## Parameters
`FileDiagnosticsRequest`, `src/server/requests.rs:200`.

| name | type | default | required |
| --- | --- | --- | --- |
| `relative_path` | string | — | yes |
| `start_line` | u32, 1-based inclusive | whole file | no |
| `end_line` | u32, 1-based inclusive | whole file | no |
| `min_severity` | u32, 1 error / 2 warning / 3 information / 4 hint | 2 | no |
| `refresh` | bool | false | no |

`min_severity` is validated by `severity_from_input` (`src/lsp/queries/diagnostics.rs:212`): anything outside 1..=4 is refused; `None` becomes 2.

## Implementation
Handler `src/server/mod.rs:451`. It parses severity, builds `SymbolQuery::new(&self.inner.language_server)`, and calls `SymbolQuery::file_diagnostics` (`src/lsp/queries/diagnostics.rs:35`).

`file_diagnostics` does three things in order:
1. `handle.session()` — starts or reuses the luau-lsp session (replaces it if the child died).
2. `handle.sync_disk_changes(&session)` — the stale-diagnostics fix. Without it luau-lsp answers about this file using whatever text it last read for the file's *dependencies*, so an edit in another module never shows up here. The sweep walks every project Luau file, diffs size+mtime stamps against the last sweep, resends open documents as `textDocument/didChange` and everything else as `workspace/didChangeWatchedFiles` events, which is what makes the server drop cached analysis of a file and of everything requiring it. Governed by `lsp.sync_disk_changes` (default true, `src/config.rs:234`); the baseline is seeded when the session starts, so the first sweep reports edits from then on rather than the whole project. Failures are logged and swallowed, never surfaced.
3. `diagnostics_of` — resolves the path, `ensure_luau_file`, opens (`session.reload` when `refresh`, else `session.ensure_open`), validates the line range, requests `textDocument/diagnostic`, and fetches the document symbol tree via `handle.document_symbols` (persistent symbol cache, `src/lsp/cache.rs`, keyed on size+mtime, honours `tools.symbol_cache`).

Filtering: a diagnostic is kept when `Severity::from_code(severity) <= min_severity` and, when a range was given, when its range overlaps `[start_line-1, end_line-1]` in 0-based LSP lines (a missing end becomes `u32::MAX`).

Grouping (`group_diagnostics`, `src/lsp/queries/diagnostics.rs:177`): result shape is `{ relative_path: { severity_label: { symbols: { name_path: [entry] }, unscoped: [entry] } } }`. A diagnostic lands under `symbols` when `SymbolNode::innermost_at` finds a symbol containing its start, otherwise under `unscoped` — never a `<file>` pseudo-symbol. Each entry is `{ line (1-based), message, code? }`; severity is deliberately not repeated inside the entry since it is the map key. Empty `symbols`/`unscoped` maps are skipped in serialization, so a clean file serializes as `{}`.

## Edge cases
- `refresh` exists for an edit that keeps a file's byte length inside one filesystem clock tick, where the size+mtime stamp says nothing moved. It forces a re-read of the target file only.
- Line bounds are checked, never clamped (`check_line_range`, `src/lsp/queries/mod.rs:283`): line 0, a line past EOF, or `start_line > end_line` are errors, not empty answers.
- Non-`.luau`/`.lua` paths are refused by `ensure_luau_file`.
- If `document_symbols` fails, symbols default to empty and every diagnostic falls into `unscoped`.
- The serialized result is capped by `tools.max_answer_chars` (default 50000, `src/config.rs:254`) in `Biskit::ok`; over the cap the call errors with an overrun hint rather than truncating.
- Session start can fail on a missing or misconfigured luau-lsp; the error carries a hint.

Siblings: `mem:Tools/get_symbol_diagnostics`, `mem:Tools/get_type_definition`, `mem:Tools/get_inlay_hints`, `mem:Tools/get_signature_help`.

## Relevant files
- `src/server/mod.rs` — `#[tool]` handler, `LANGUAGE_SERVER_TOOLS` gating, `ok`/answer cap
- `src/server/requests.rs` — `FileDiagnosticsRequest`
- `src/server/descriptions.rs` — tool description macro arm
- `src/lsp/queries/diagnostics.rs` — `file_diagnostics`, `diagnostics_of`, `group_diagnostics`, `severity_from_input`
- `src/lsp/queries/mod.rs` — `SymbolQuery`, `check_line_range`, `check_line_bound`
- `src/lsp/session/handle.rs` — `session`, `sync_disk_changes`, `document_symbols`
- `src/lsp/session/mod.rs` — `ensure_open`, `reload`, `sync_disk_changes`, `diagnostics`
- `src/lsp/cache.rs` — `SymbolCache`, `SourceStamp`
- `src/lsp/protocol.rs` — `Diagnostic`, `Severity`
- `src/lines.rs` — `LineIndex`
- `src/config.rs` — `lsp.sync_disk_changes`, `tools.max_answer_chars`, `tools.symbol_cache`, `project.memory_only`
