# Biskit Instructions Manual

Biskit = symbolic code-intelligence and project-memory server for this Luau project. Read-only on source: never edit, create, delete source files. Use own native write tools for all edits.

## Two responsibilities

1. **Project memory** — durable curated notes about project, survive between sessions.
2. **Luau code intelligence** — symbol lookup, references, diagnostics from real language server, not text search.

## Start of every session

Call `list_memories` first. Returns memory names only. Read ones whose names relate to task with `read_memory`. Not every memory.

## Choosing a tool

| Goal | Tool |
|---|---|
| Understand a file's structure before reading it | `get_symbols_overview` |
| Read one symbol's implementation | `find_symbol` with `include_body: true` |
| Locate a symbol anywhere in the project | `find_symbol` |
| Jump to where a symbol is defined | `find_declaration` |
| Find every caller or user of a symbol | `find_referencing_symbols` |
| Check whether a file type-checks | `get_file_diagnostics` |
| Check a symbol and its callers for breakage after an edit | `get_symbol_diagnostics` |
| Find files by name or glob | `find_file` |
| See what is in a directory | `list_dir` |
| Regex search across file contents | `search_for_pattern` |

Prefer symbolic tools over whole files. Reading 900-line module for one function burn context rest of task need. `get_symbols_overview` then targeted `find_symbol` almost always cheaper.

`search_for_pattern` = right tool for non-symbol text: string literals, comments, config keys, remote event names. Wrong tool for finding function definition — use `find_symbol`.

## Name paths

Symbols addressed by name path: separated chain of enclosing symbol names. `/`, `.`, and `:` all work as separator, so write name the way it appear in source.

- `update` matches any symbol named `update` at any depth.
- `PlayerService/update` matches `update` nested directly inside `PlayerService`.
- `PlayerService.update` and `PlayerService:update` same thing.
- `/PlayerService` matches only top-level `PlayerService`, not nested one.
- `/PlayerService/update` fully absolute.

Method declared `function PlayerUtils:GetPlayerMaid()` addressable as `GetPlayerMaid`, `PlayerUtils:GetPlayerMaid`, or `PlayerUtils/GetPlayerMaid`. Owner name not required.

Set `substring_matching: true` to match final segment loosely when you know only part of name.

When file has two symbols of same name, `get_symbols_overview` labels them `UserInfo[0]` and `UserInfo[1]`. Pass label back verbatim to address exactly one. Bare `UserInfo` matches both. Tools taking single symbol (`find_declaration`, `find_referencing_symbols`, `get_symbol_diagnostics`) error on ambiguous name — use indexed form there.

`find_symbol` returns `{ symbols, truncated }`. `truncated: true` means `max_matches` cut result short: narrow with `relative_path` or raise cap. Field omitted entirely when nothing was cut, so absent = complete. Same for `list_dir` and `search_for_pattern`.

`symbols` is keyed by file path, and each symbol under it carries no path of its own — path comes from key it sits under. `find_declaration` returns same file-keyed shape. `get_symbols_overview` returns bare list, since you supplied file yourself.

Top-level symbol in result carries full name path. Nested symbol under `children` carries only own leaf name, because ancestry already spelled by chain it sits under. Join with `/` to address it: child `update` under `PlayerService` is `PlayerService/update`.

## After you edit code

Biskit read source from disk each request, so edits visible immediately. After non-trivial edit, call `get_file_diagnostics` on changed file. If edit changed symbol signature or behavior, call `get_symbol_diagnostics` with `check_symbol_references: true` to catch breakage at call sites.

## Writing memories

Write memory when you learn something future session would otherwise rediscover: architectural decisions plus reasoning, non-obvious invariants, where subsystem lives, why workaround exists, project conventions.

Do not write memory for: file contents (read it instead), transient task state, anything already in `CLAUDE.md` or `AGENTS.md`, summary of work you just did.

Give memories meaningful names. Nest with `/` when topic has several parts, example `Combat/HitDetection`. Cross-reference other memories with `mem:` pointer in backticks, such as `` `mem:Combat/HitDetection` ``. `rename_memory` rewrites those pointers automatically.

Use `edit_memory` to amend existing memory, not wholesale rewrite with `create_memory`. Wholesale rewrites lose detail that was there for reason. `create_memory` errors when name already taken; pass `overwrite: true` only when replacing content deliberately.

## Diagnostics severity

`min_severity` filters results: `1` errors only, `2` errors and warnings, `3` adds information, `4` adds hints. Default `2`. Ask `1` when you care only whether something broken.

Diagnostics grouped file, then severity, then `symbols` keyed by name path. Diagnostics belonging to no symbol land in `unscoped` list beside it.

## When the language server misbehaves

If symbol tools return empty results for file you know has symbols, or diagnostics look stale against file you just changed, call `restart_language_server`. Cheap. Do not restart reflexively for empty result that means only "no matches" — verify with `get_symbols_overview` first.
