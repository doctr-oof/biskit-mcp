Finds where a symbol is declared, addressed either by a name path inside the named file or by the line/column of a use of it.

Disabled while `project.memory_only` is true: the route is removed at startup (`src/server/mod.rs:134`, list at `:96`).

## Parameters

Struct `FindDeclarationRequest`, `src/server/requests.rs:177`.

- `name_path` — `Option<String>`, default `None`. Must name a symbol **the named file itself declares**. Accepts a `[n]` suffix to pick between same-named siblings.
- `relative_path` — `String`, required. File containing the symbol or the use.
- `line` — `Option<u32>`, default `None`. 1-based line of a *use* of the symbol. Mutually exclusive with `name_path`.
- `column` — `Option<u32>`, default `None` → `1`. 1-based, counted in UTF-16 code units.
- `include_body` — `bool`, default `false`. Source snippet around each result.
- `include_detail` — `bool`, default `false`. Type signature per result; one hover request each.

## Implementation

Handler `Biskit::find_declaration`, `src/server/mod.rs:406`.

1. `SymbolPoint::parse(name_path, line, column)` (`src/lsp/queries/mod.rs:64`) turns the three fields into either `NamePath` or `LineColumn`. Passing both `name_path` and `line` is refused; passing neither is refused; `line: 0` is refused; a blank/whitespace `name_path` is treated as absent, so `name_path: "  "` plus `line` resolves to the line.
2. `SymbolQuery::find_declaration` (`src/lsp/queries/symbols.rs:197`).

`find_declaration`:
- `locate_point` (`src/lsp/queries/mod.rs:192`). For a name path this is `locate_one` (`:143`), which parses the pattern with `substring_matching: false`, walks the file's symbol tree, and requires exactly one hit; the LSP position used is `SymbolNode::target_position` (`src/lsp/symbols.rs:35`), which seeks past the owner table to the identifier itself. For a line/column it bounds-checks the line against the file and the column against the line's UTF-16 width, then records the innermost symbol containing that position (may be `None`).
- Issues `textDocument/definition` at the resolved position (`src/lsp/session/mod.rs:493`).
- If the server returns locations, `render_locations` (`src/lsp/queries/references.rs:259`) groups them by file, names each by the innermost symbol at the location, and returns `SymbolsByFile` (a `BTreeMap<relative_path, Vec<SymbolMatch>>`).
- If the server returns **nothing** but the point resolved to a symbol, the symbol itself is rendered and returned — i.e. asking about a declaration site answers with that declaration rather than erroring.

Invariants:
- A call site is not a document symbol, so a name path only ever resolves inside the file that declares the symbol. Following a symbol into another file requires `line`/`column` at a use of it — this is stated both in the tool description (`src/server/descriptions.rs:50`) and in `DECLARATION_HINT` (`src/lsp/queries/symbols.rs:28`).
- `include_body` here is a **snippet**, not the symbol's full body: `render_locations` uses `snippet_around` with `DECLARATION_CONTEXT_LINES = 1` (`src/lsp/queries/references.rs:22`). Only the no-location fallback path renders a real body via `render`.
- `end_line` in a rendered location is the declaration's end line when the innermost symbol starts on the same line, otherwise the location range's own end.
- When no symbol covers a location, `name_path` is omitted and `kind` is `"Unknown"`.

## Edge cases

- `name_path` with no match: refused, with a hint that name paths are case-sensitive and that `get_symbols_overview` shows what the file defines.
- `name_path` ambiguous (several matches): refused, listing every matching chain and suggesting the `[0]` index form.
- No definition and no symbol at the point: refused with `DECLARATION_HINT` naming the resolved `file:line:column`.
- Every resolved location outside the project root: `render_locations` bails with "the language server resolved this outside the project root", naming the paths.
- Locations whose URI is not a readable `file://` path are silently dropped by `group_locations_by_file` (`src/lsp/queries/render.rs:19`).
- `relative_path` must be `.luau`/`.lua` (`ensure_luau_file`, `src/lsp/session/handle.rs:245`).
- Line past end of file, or column past the line's UTF-16 width, are both refused with the real bound in the message (`check_line_bound`, `src/lsp/queries/mod.rs:255`; `COLUMN_HINT`, `:43`). Nothing is clamped.
- Detail budget is `MAX_DETAIL_HOVERS = 200` shared across the answer; hovers that fail leave `detail` absent.
- Answer capped by `tools.max_answer_chars` (default `50_000`), erroring rather than truncating.

## Relevant files

- `src/server/mod.rs` — handler (`:406`), memory-only gating (`:96`, `:134`)
- `src/server/requests.rs` — `FindDeclarationRequest` (`:177`)
- `src/server/descriptions.rs` — tool description (`:50`)
- `src/lsp/queries/symbols.rs` — `find_declaration` (`:197`), `DECLARATION_HINT` (`:28`)
- `src/lsp/queries/mod.rs` — `SymbolPoint::parse` (`:64`), `locate_one` (`:143`), `locate_point` (`:192`), `check_line_bound` (`:255`), hints (`:36`–`:51`)
- `src/lsp/queries/references.rs` — `render_locations` (`:259`), `DECLARATION_CONTEXT_LINES` (`:22`)
- `src/lsp/queries/render.rs` — `group_locations_by_file` (`:19`), `snippet_around` (`:111`), `render`
- `src/lsp/symbols.rs` — `target_position` (`:35`), `innermost_at` (`:60`), `find_identifier` (`:90`)
- `src/lsp/session/mod.rs` — `definition` (`:493`), `hover` (`:527`), `ensure_open` (`:235`)
- `src/lsp/session/handle.rs` — `session` (`:92`), `document_symbols` (`:57`), `ensure_luau_file` (`:245`)
- `src/lsp/uri.rs` — `from_path`/`to_path`
- `src/config.rs` — `project.memory_only` (`:152`), `tools.max_answer_chars` (`:161`)

Siblings: `mem:Tools/get_symbols_overview`, `mem:Tools/find_symbol`, `mem:Tools/find_referencing_symbols`, `mem:Tools/explain_symbol`.
