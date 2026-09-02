Finds every reference to a symbol, reported with the symbol each reference sits inside.

Disabled while `project.memory_only` is true: the route is removed at startup (`src/server/mod.rs:134`, list at `:96`).

## Parameters

Struct `SymbolLocationRequest`, `src/server/requests.rs:166`. This tool takes a name path only — there is no line/column form.

- `name_path` — `String`, required. Name path of the symbol, resolved inside `relative_path`. Accepts a `[n]` suffix to pick between same-named siblings.
- `relative_path` — `String`, required. The file that declares the symbol.
- `context_lines` — `usize`, default `0`. Source lines shown either side of each reference; `0` shows the reference line alone.

The result cap is not a parameter: `tools.max_reference_matches` (default `200`, `src/config.rs:257`) is passed in by the handler at `src/server/mod.rs:441`.

## Implementation

Handler `Biskit::find_referencing_symbols`, `src/server/mod.rs:430` → `SymbolQuery::find_referencing_symbols` (`src/lsp/queries/references.rs:118`) → `references_at` (`:138`).

1. `locate_one` (`src/lsp/queries/mod.rs:143`) resolves the name path to exactly one `SymbolNode` in the named file and to the identifier position via `target_position`. Zero matches and ambiguous matches are both refused.
2. `textDocument/references` with `includeDeclaration: false` (`src/lsp/session/mod.rs:572`).
3. `is_declaration_site` (`src/lsp/queries/references.rs:385`) additionally drops any location that is exactly the queried position in the queried file, since some servers report it regardless.
4. **`self:` recovery.** `self_receiver_locations` (`:229`) runs only when the symbol's name path contains `/` (i.e. it is a member). It blanks comments (`roblox::requires::blank_comments`), then `self_receiver_positions` (`:344`) scans the declaring file for `self:Name` / `self.Name` with identifier-boundary checks on both sides, skipping lines the server already reported. This exists because luau-lsp types the implicit `self` of a colon-declared method as a fresh generic rather than as the owner, so it reports no reference for those call sites.
5. Locations are grouped by file in first-seen order (`group_locations_by_file`); the declaring file's group is re-sorted by (line, character) when any recovered position was merged in.
6. Each reference becomes a `ReferenceMatch` (`:39`): 1-based `line`, `containing_symbol` from `SymbolNode::innermost_at`, `snippet` from `snippet_around`, and `resolved_by: "text"` on recovered ones.
7. Probe count is `max_reference_matches + 1`; the extra sets `truncated` before truncation. Final grouping is a `BTreeMap` keyed by relative path.

## Edge cases

- `note` is set to `SELF_REFERENCE_NOTE` (`src/lsp/queries/references.rs:29`) only when a text-recovered reference actually survived truncation. Those are matched on the symbol's **name alone**, so the receiver must be confirmed before treating one as a genuine call site.
- Text recovery covers the declaring file only, and only for member symbols. A method on a non-member symbol gets no recovery.
- `containing_symbol` is omitted when no symbol's range covers the reference (module-level code).
- Files whose symbols cannot be fetched, or which cannot be relativized against the project root, are skipped silently — a reference in an out-of-root file simply does not appear.
- Truncation counts references, not files: the loop breaks mid-file once the probe is reached, so a file may appear partially covered.
- `truncated` and `note` are omitted from the JSON when false/absent, so absent means complete.
- This tool sees **symbol uses**, not module-level coupling. `get_require_graph` is the documented complement (`src/server/descriptions.rs:90`).
- Answer capped by `tools.max_answer_chars` (default `50_000`), erroring rather than truncating; a large `context_lines` on a widely used symbol will reach it before `max_reference_matches` does.
- `relative_path` must be `.luau`/`.lua`.

## Relevant files

- `src/server/mod.rs` — handler (`:430`), `max_reference_matches` wiring (`:441`), memory-only gating (`:96`, `:134`)
- `src/server/requests.rs` — `SymbolLocationRequest` (`:166`)
- `src/server/descriptions.rs` — tool description (`:54`)
- `src/lsp/queries/references.rs` — `find_referencing_symbols` (`:118`), `references_at` (`:138`), `self_receiver_locations` (`:229`), `self_receiver_positions` (`:344`), `is_declaration_site` (`:385`), `ReferenceMatch` (`:39`), `ReferenceSearchResult` (`:52`), `SELF_REFERENCE_NOTE` (`:29`)
- `src/lsp/queries/mod.rs` — `locate_one` (`:143`), `group_by_file` (`:306`)
- `src/lsp/queries/render.rs` — `group_locations_by_file` (`:19`), `snippet_around` (`:111`)
- `src/lsp/symbols.rs` — `innermost_at` (`:60`), `target_position` (`:35`), `is_identifier_byte` (`:85`)
- `src/lsp/session/mod.rs` — `references` (`:572`), `ensure_open` (`:235`)
- `src/lsp/session/handle.rs` — `session` (`:92`), `document_symbols` (`:57`)
- `src/roblox/requires.rs` — `blank_comments`
- `src/lsp/uri.rs` — `from_path`/`to_path`
- `src/config.rs` — `tools.max_reference_matches` (`:164`, default `:257`)

Siblings: `mem:Tools/get_symbols_overview`, `mem:Tools/find_symbol`, `mem:Tools/find_declaration`, `mem:Tools/explain_symbol`.
