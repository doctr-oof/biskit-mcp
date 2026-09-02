Reports the type the language server *infers* at a position — not the type written in source — plus optionally the doc comment.

Disabled while `project.memory_only` is true: the route is removed at startup (`src/server/mod.rs:134`, list at `:96`).

## Parameters

Struct `ExplainSymbolRequest`, `src/server/requests.rs:235`.

- `name_path` — `Option<String>`, default `None`. Resolved against symbols the named file declares. Mutually exclusive with `line`.
- `relative_path` — `String`, required.
- `line` — `Option<u32>`, default `None`. 1-based; aim it at a *use* of the symbol.
- `column` — `Option<u32>`, default `None` → `1`. 1-based, UTF-16 code units.
- `include_documentation` — `bool`, default `false`.

## Implementation

Handler `Biskit::explain_symbol`, `src/server/mod.rs:499` → `SymbolPoint::parse` (`src/lsp/queries/mod.rs:64`) → `SymbolQuery::explain_symbol` (`src/lsp/queries/hover.rs:95`).

1. `locate_point` resolves the point exactly as `mem:Tools/find_declaration` does — name path through `locate_one` (unique match required, position from `target_position`), or line/column with bounds checks and the innermost containing symbol recorded.
2. `textDocument/hover` at that position (`src/lsp/session/mod.rs:527`).
3. `split_hover` (`src/lsp/queries/hover.rs:297`) splits the hover markdown into the fenced code half (the signature) and the prose half (the documentation), dropping horizontal rules. When the markdown carries **no fence at all**, the whole thing is returned as the signature and the documentation is empty.
4. `strip_unbound_generics` (`:323`) removes generic parameters from the signature that appear nowhere outside the generic list.
5. Result assembly depends on how the point was given — this is the key asymmetry:
   - **`name_path` form**: `name_path` and `kind` come from the located symbol; `declared_in` and `containing_symbol` are always absent.
   - **`line`/`column` form**: `containing_symbol` is the innermost symbol at the position, and `name_path`/`kind`/`declared_in` come from a separate `textDocument/definition` lookup via `declaration_at` (`:176`). `declared_in` is set only when the declaration lands in a *different* file than the one asked about.

`declaration_at` deliberately does not reuse the innermost symbol: that answers "what does this position sit inside", so a call site would come back named after its caller. `declared_symbol` (`:285`) accepts a location only when it falls in the node's `selection_range` or at the node's range start.

Result type `SymbolExplanation` (`src/lsp/queries/hover.rs:27`): `relative_path`, 1-based `line`/`column`, optional `name_path`, `declared_in`, `kind`, `containing_symbol`, required `signature`, optional `documentation`, optional `note`.

## Edge cases

- `SymbolPoint::parse` refuses both `name_path` and `line` together, refuses neither, and refuses `line: 0`. A whitespace-only `name_path` is treated as absent, so it falls through to the line form.
- Hover with an empty signature *and* empty documentation: refused with `POINT_HINT` naming the resolved `file:line:column`. A hover the server does not answer at all yields the same empty markdown and the same refusal.
- Declaration resolving outside the project root: `name_path`, `kind` and `declared_in` are all absent and `note = OUTSIDE_ROOT_NOTE` (`:14`) is set. `containing_symbol` is still reported.
- Declaration unresolvable (`definition` errored or returned nothing, or no node matched): all four fields are simply absent, with no note — absent here is ambiguous between "not found" and "not applicable".
- `documentation` is capped at `MAX_DOCUMENTATION_CHARS = 4_000` chars (`:12`), with `\n\n[documentation truncated]` appended (`cap_documentation`, `:424`). An empty documentation string is dropped rather than serialised as `""`.
- Name path with zero or several matches: refused by `locate_one`, listing the candidates for the ambiguous case.
- Line past end of file or column past the line's UTF-16 width: refused with the real bound; nothing is clamped.
- `relative_path` must be `.luau`/`.lua`.
- Answer capped by `tools.max_answer_chars` (default `50_000`).
- Complements: `find_symbol` with `include_detail` reports declared signatures, while this reports the resolved type (`src/lsp/queries/inlay.rs:90`); when a build lacks `textDocument/signatureHelp`, `get_signature_help` points callers here instead (`src/lsp/queries/hover.rs:225`).

## Relevant files

- `src/server/mod.rs` — handler (`:499`), memory-only gating (`:96`, `:134`)
- `src/server/requests.rs` — `ExplainSymbolRequest` (`:235`)
- `src/server/descriptions.rs` — tool description (`:65`)
- `src/lsp/queries/hover.rs` — `explain_symbol` (`:95`), `declaration_at` (`:176`), `declared_symbol` (`:285`), `split_hover` (`:297`), `strip_unbound_generics` (`:323`), `cap_documentation` (`:424`), `SymbolExplanation` (`:27`), `DeclarationSite` (`:51`), `OUTSIDE_ROOT_NOTE` (`:14`)
- `src/lsp/queries/mod.rs` — `SymbolPoint::parse` (`:64`), `locate_one` (`:143`), `locate_point` (`:192`), `POINT_HINT` (`:40`), `COLUMN_HINT` (`:43`)
- `src/lsp/queries/render.rs` — `group_locations_by_file` (`:19`)
- `src/lsp/symbols.rs` — `SymbolNode`, `target_position` (`:35`), `innermost_at` (`:60`)
- `src/lsp/session/mod.rs` — `hover` (`:527`), `definition` (`:493`), `ensure_open` (`:235`)
- `src/lsp/session/handle.rs` — `session` (`:92`), `document_symbols` (`:57`), `ensure_luau_file` (`:245`)
- `src/lsp/uri.rs` — `to_path`
- `src/config.rs` — `project.memory_only` (`:152`), `tools.max_answer_chars` (`:161`)

Siblings: `mem:Tools/get_symbols_overview`, `mem:Tools/find_symbol`, `mem:Tools/find_declaration`, `mem:Tools/find_referencing_symbols`.
