Finds symbols by name path across the whole project, or within one file or directory.

Disabled while `project.memory_only` is true: the route is removed at startup (`src/server/mod.rs:134`, list at `:96`).

## Parameters

Struct `FindSymbolRequestInput`, `src/server/requests.rs:130`.

- `name_path` — `String`, required. See name-path syntax below.
- `relative_path` — `Option<String>`, default `None`. A file or a directory. Omitted means the whole project.
- `depth` — `u32`, default `0`. Levels of children rendered under each match.
- `include_body` — `bool`, default `false`. Source text of each matched symbol.
- `include_detail` — `bool`, default `false`. Resolved type signature per symbol; one hover request each.
- `include_locals` — `bool`, default `false`. Keep variables declared inside function bodies.
- `include_kinds` — `Vec<u32>`, default empty (all kinds). LSP `SymbolKind` numbers to keep.
- `exclude_kinds` — `Vec<u32>`, default empty. Kinds to drop; applied after `include_kinds`.
- `substring_matching` — `bool`, default `false`. Matches the **final** name-path segment as a substring only.
- `max_matches` — `usize`, default `50` (`default_max_matches`, `src/server/requests.rs:424`), clamped down by `tools.max_listing_entries` (default `2_000`) at `src/server/mod.rs:395`.

## Name-path syntax and matching

`NamePathPattern::parse` (`src/lsp/name_path.rs:10`) and `matches` (`:44`).

- `/`, `.` and `:` are interchangeable separators, so `PlayerService/update`, `PlayerService.update` and `PlayerService:update` all address the same symbol.
- A leading `/` or `.` makes the pattern **absolute**: it must match starting at the file's top level. `/PlayerService` matches `PlayerService` but not `Outer/PlayerService`.
- A relative pattern matches at any depth, but its segments must be a **contiguous suffix** of the symbol's ancestor chain. `PlayerService/update` matches `Module/PlayerService/update` but not `PlayerService/Inner/update`.
- Matching is case-sensitive and exact per segment, except the last segment when `substring_matching` is set.
- `[n]` disambiguates same-named siblings. A query segment carrying `[n]` requires an exact match including the suffix; a bare query segment matches every duplicate, because `strip_overload_suffix` (`src/lsp/name_path.rs:78`) drops the suffix from the candidate first. `[abc]` is not a suffix — only all-digit brackets count.
- An empty pattern, or one that matches an empty chain, matches nothing.

## Implementation

Handler `Biskit::find_symbol`, `src/server/mod.rs:373`.

1. Validates both kind lists with `check_symbol_kinds` (`src/lsp/queries/symbols.rs:243`) before touching the language server.
2. Clamps `max_matches` to `tools.max_listing_entries`.
3. Calls `SymbolQuery::find_symbol` (`src/lsp/queries/symbols.rs:101`).

`find_symbol` itself:
- Refuses an empty parsed pattern with `NAME_PATH_HINT`.
- `SymbolQuery::candidate_files` (`src/lsp/queries/mod.rs:122`): a file path is used alone (after `ensure_luau_file`), a directory is walked, an absent `relative_path` sweeps the project via `resolve_luau_files` (`src/lsp/session/handle.rs:214`, `.luau` and `.lua`, sorted, honouring the project ignore set).
- `prefilter_by_literal` (`src/lsp/queries/symbols.rs:259`) drops candidate files whose raw bytes never spell the pattern's leaf name (`literal_filter`, suffix stripped). Skipped when only one file is in play. A file that cannot be read is **kept**, so it is still reported.
- Walks each file's symbol tree with `collect_matches` (`:285`), which recurses into children regardless of whether the parent matched, so nested matches are found even under a non-matching owner.
- Probe count is `max_matches + 1`; the extra match is what sets `truncated` before the vector is truncated to `max_matches`.
- `include_detail` runs `attach_details` per file against a single shared `budget` of `MAX_DETAIL_HOVERS = 200` across the whole answer.
- Results are grouped by relative file path into a `BTreeMap` (`SymbolsByFile`), so file order in the answer is lexicographic even though the scan order was not.

## Edge cases

- A kind number outside `1..=26` is refused with a hint spelling out the whole kind table (`SYMBOL_KIND_HINT`, `src/lsp/queries/symbols.rs:35`). This exists specifically because an out-of-range kind would otherwise return an empty answer that reads like "no such symbol".
- Kind filtering is applied to the **matched** node only; `omitted_children` counts children dropped by the low-level-kind prune in `render`, not by `include_kinds`/`exclude_kinds`.
- `include_body` applies to top-level matches only — `render_child` (`src/lsp/queries/render.rs:46`) forces `include_body: false` for descendants.
- A `relative_path` that is neither file nor directory is refused with a hint pointing at `find_file`/`list_dir`.
- If the language server stops answering mid-scan, the error is wrapped with `SCAN_ABORTED` (`src/lsp/queries/symbols.rs:19`) telling the caller to `restart_language_server`. Other per-file errors are skipped silently.
- `truncated: true` and `note` are omitted from the JSON when false/absent, so absent means complete.
- Whole answer is capped by `tools.max_answer_chars` (default `50_000`) and errors when over — an unqualified `name_path` with `include_body` across a large project will hit it.

## Relevant files

- `src/server/mod.rs` — handler (`:373`), memory-only gating (`:96`, `:134`)
- `src/server/requests.rs` — `FindSymbolRequestInput` (`:130`), `default_max_matches` (`:424`)
- `src/server/descriptions.rs` — tool description (`:47`)
- `src/lsp/queries/symbols.rs` — `find_symbol` (`:101`), `FindSymbolRequest` (`:87`), `collect_matches` (`:285`), `prefilter_by_literal` (`:259`), `check_symbol_kinds` (`:243`), `SymbolSearchResult` (`:68`)
- `src/lsp/name_path.rs` — `NamePathPattern`, `strip_overload_suffix`
- `src/lsp/queries/mod.rs` — `candidate_files` (`:122`), `attach_details` (`:314`), `group_by_file` (`:306`), `NAME_PATH_HINT` (`:36`)
- `src/lsp/queries/render.rs` — `render`, `render_child`, `RenderOptions`
- `src/lsp/symbols.rs` — `SymbolNode`, `build_tree`
- `src/lsp/session/handle.rs` — `resolve_luau_files` (`:214`), `document_symbols` (`:57`), `session` (`:92`)
- `src/config.rs` — `tools.max_listing_entries`, `tools.max_answer_chars` (`:159`, `:250`)

Siblings: `mem:Tools/get_symbols_overview`, `mem:Tools/find_declaration`, `mem:Tools/find_referencing_symbols`, `mem:Tools/explain_symbol`.
