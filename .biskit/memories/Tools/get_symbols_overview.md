Lists the symbols one Luau file defines, so a caller can decide what is worth reading before reading it.

Disabled while `project.memory_only` is true: the handler is removed from the tool router at startup (`src/server/mod.rs:134`) along with the rest of `LANGUAGE_SERVER_TOOLS` (`src/server/mod.rs:96`), so the tool is not registered at all in that mode.

## Parameters

Struct `SymbolsOverviewRequest`, `src/server/requests.rs:112`.

- `relative_path` — `String`, required. Luau source file relative to the project root.
- `depth` — `u32`, default `1` (`default_overview_depth`, `src/server/requests.rs:428`). `0` lists top-level symbols only; `1` reaches the members of a table.
- `include_detail` — `bool`, default `false`. Fills each symbol's resolved type signature; costs one hover request per symbol.
- `include_locals` — `bool`, default `false`. Descends into variables declared inside function bodies.

## Implementation

Handler `Biskit::get_symbols_overview`, `src/server/mod.rs:352`. Constructs `SymbolQuery::new(&language_server)` and calls `SymbolQuery::symbols_overview`, `src/lsp/queries/symbols.rs:156`.

That function:
1. `Project::resolve(relative_path)` then `ensure_luau_file` (`src/lsp/session/handle.rs:245`) — only `.luau` and `.lua` are accepted.
2. `LanguageServerHandle::session()` (`src/lsp/session/handle.rs:92`) — starts or reuses the luau-lsp session; this is the call that refuses in memory-only mode.
3. `LanguageServerHandle::document_symbols` (`src/lsp/session/handle.rs:57`) — served from the on-disk symbol cache under `.biskit/cache/` when the file's `SourceStamp` is unchanged, otherwise `textDocument/documentSymbol`.
4. Renders each root node through `render` (`src/lsp/queries/render.rs:38`) with `RenderOptions { depth, include_body: false, include_detail, include_locals }`.
5. When `include_detail` is set, `attach_details` (`src/lsp/queries/mod.rs:314`) walks the rendered tree and issues one `textDocument/hover` per symbol, taking the fenced code half of the hover through `split_hover` and then `strip_unbound_generics`.

Invariants:
- `include_body` is hard-wired `false` here. This tool never returns source text; `mem:Tools/find_symbol` is where `include_body` lives.
- Result shape is `SymbolOverviewResult { symbols: Vec<SymbolMatch>, note }` (`src/lsp/queries/symbols.rs:80`) — a flat vector for the one file, not the by-file map the search tools return.
- Root `SymbolMatch.name_path` carries the full `/`-joined ancestor chain; nested children carry the bare name only (`render_node`'s `full_name_path` flag, `src/lsp/queries/render.rs:54`).
- `start_line`/`end_line` are 1-based; the LSP ranges are converted by `+ 1`.
- Symbol name paths are built by `build_tree` (`src/lsp/symbols.rs:114`): dotted and colon Luau names become path segments, and same-named siblings get a `[n]` suffix.

## Edge cases

- Not a `.luau`/`.lua` file: refused by `ensure_luau_file` with a hint pointing at `find_file` with mask `"*.luau"`.
- Detail budget: `MAX_DETAIL_HOVERS = 200` (`src/lsp/queries/mod.rs:53`). Past it, symbols keep `detail: None` and the result carries `note = DETAIL_CAPPED_NOTE` (`src/lsp/queries/symbols.rs:22`) advising a lower depth or `explain_symbol`.
- With `include_locals` off, low-level kinds (LSP kinds 13,14,15,16,17,18,19,20,21,26 — `is_low_level_kind`, `src/lsp/protocol.rs:336`) are pruned from children and counted into `omitted_children`. Symbols flagged `member` are kept either way.
- At `depth: 0` nothing is descended into, so `omitted_children` is always `0` — an absent count does not mean the symbol has no children.
- A hover that fails or returns an empty signature leaves `detail` absent without erroring.
- Whole-result size is capped by `tools.max_answer_chars` (default `50_000`); over it the call errors rather than truncating, because `Biskit::ok` serialises JSON (`src/server/mod.rs:59`).
- If the file has no symbols at all the answer is an empty vector, which is indistinguishable from a stale index — `restart_language_server` is the documented remedy.

## Relevant files

- `src/server/mod.rs` — handler (`:352`), memory-only gating (`:96`, `:134`), answer-size cap (`:57`)
- `src/server/requests.rs` — `SymbolsOverviewRequest` (`:112`), `default_overview_depth` (`:428`)
- `src/server/descriptions.rs` — tool description (`:43`)
- `src/lsp/queries/symbols.rs` — `symbols_overview` (`:156`), `SymbolOverviewResult` (`:80`), `SymbolMatch` (`:43`), `DETAIL_CAPPED_NOTE` (`:22`)
- `src/lsp/queries/mod.rs` — `SymbolQuery` (`:109`), `attach_details` (`:314`), `MAX_DETAIL_HOVERS` (`:53`)
- `src/lsp/queries/render.rs` — `render`/`render_node`, `RenderOptions`
- `src/lsp/symbols.rs` — `SymbolNode`, `build_tree`, name-path construction
- `src/lsp/protocol.rs` — `is_low_level_kind` (`:336`), `symbol_kind_label`
- `src/lsp/session/handle.rs` — `document_symbols` (`:57`), `session` (`:92`), `ensure_luau_file` (`:245`)
- `src/lsp/session/mod.rs` — `document_symbols` (`:481`), `hover` (`:527`)
- `src/config.rs` — `ProjectSettings::memory_only` (`:152`), `ToolSettings` (`:159`) and defaults (`:250`)

Siblings: `mem:Tools/find_symbol`, `mem:Tools/find_declaration`, `mem:Tools/find_referencing_symbols`, `mem:Tools/explain_symbol`.
