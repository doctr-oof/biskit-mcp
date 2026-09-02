Reports diagnostics for one named symbol and, optionally, for every file that references it (declaring file included). Disabled while `project.memory_only` is true — the route is removed in `src/server/mod.rs:96` and `LanguageServerHandle::session` refuses anyway.

## Parameters
`SymbolDiagnosticsRequest`, `src/server/requests.rs:218`.

| name | type | default | required |
| --- | --- | --- | --- |
| `name_path` | string | — | yes |
| `relative_path` | string | — | yes |
| `check_symbol_references` | bool | false | no |
| `min_severity` | u32, 1..=4 | 2 | no |
| `refresh` | bool | false | no |

`name_path` resolves only against symbols the named file declares. Same-named siblings take an index suffix, e.g. `Foo/bar[0]`.

## Implementation
Handler `src/server/mod.rs:475` → `SymbolQuery::symbol_diagnostics` (`src/lsp/queries/diagnostics.rs:106`).

Order of work:
1. `handle.session()`, then `handle.sync_disk_changes(&session)` — same stale-diagnostics sweep described in `mem:Tools/get_file_diagnostics`; it runs once for the whole call, and the per-file reads afterwards go through `diagnostics_of`, which never re-sweeps.
2. `locate_one` (`src/lsp/queries/mod.rs:143`) resolves `name_path` against the file's document symbol tree using `NamePathPattern` (case-sensitive, non-substring). Zero matches and ambiguous matches are both errors, the ambiguous one listing every candidate.
3. Without `check_symbol_references`: delegates to `diagnostics_of` with `start_line`/`end_line` set to the symbol's own clamped range (converted to 1-based). So it is `get_file_diagnostics` narrowed to the symbol's span.
4. With `check_symbol_references`: first sweeps the declaring file **whole** (no line range), then `session.references(path, position, include_declaration = false)`, and for every distinct resolvable target file sweeps that file whole too, merging with `merge_severities` (`src/lsp/queries/diagnostics.rs:164`). Referencing files are not clipped to the reference site.

Result shape is `GroupedDiagnostics`, identical to `get_file_diagnostics` but keyed across several files: `{ relative_path: { severity: { symbols: {...}, unscoped: [...] } } }`.

## Edge cases
- A `visited` set seeded with the declaring path stops a file being swept twice; ordering means the declaring file's entries come from the un-clipped sweep.
- Reference targets that fail `uri::to_path`, fail `relativize` (outside the project root), or whose own diagnostics call errors are silently skipped — a partial answer, never an error.
- `check_symbol_references: false` clips by line range, so a diagnostic that luau-lsp attributes to a line outside the symbol's `range` (for example a use of the symbol further down the file) is not reported.
- `refresh` is forwarded to every file swept, so with `check_symbol_references` it forces a re-read of each referencing file too.
- `min_severity` validated by `severity_from_input`; out-of-range values error.
- Answer capped by `tools.max_answer_chars`; a broad `check_symbol_references` sweep is the most likely tool here to hit it.

Siblings: `mem:Tools/get_file_diagnostics`, `mem:Tools/get_type_definition`, `mem:Tools/get_inlay_hints`, `mem:Tools/get_signature_help`.

## Relevant files
- `src/server/mod.rs` — handler, memory-only gating
- `src/server/requests.rs` — `SymbolDiagnosticsRequest`
- `src/server/descriptions.rs` — description arm
- `src/lsp/queries/diagnostics.rs` — `symbol_diagnostics`, `diagnostics_of`, `merge_severities`, `group_diagnostics`
- `src/lsp/queries/mod.rs` — `locate_one`, `check_line_range`
- `src/lsp/name_path.rs` — `NamePathPattern`
- `src/lsp/session/handle.rs` — `session`, `sync_disk_changes`, `document_symbols`
- `src/lsp/session/mod.rs` — `references`, `diagnostics`, `ensure_open`, `reload`
- `src/lsp/uri.rs` — `to_path`
- `src/lsp/symbols.rs` — `SymbolNode`, `innermost_at`, `target_position`
- `src/config.rs` — `lsp.sync_disk_changes`, `tools.max_answer_chars`, `project.memory_only`
