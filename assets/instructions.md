# Biskit Instructions Manual

Biskit = symbolic code-intelligence and project-memory server for this Luau project. Read-only on source: never edit, create, delete source files. Use own native write tools for all edits.

## Two responsibilities

1. **Project memory** — durable curated notes about project, survive between sessions.
2. **Luau code intelligence** — symbol lookup, references, diagnostics from real language server, not text search.

## Start of every session — mandatory, no exceptions

**Check project memory before you do anything else.** Before you answer a question, open a file, run a search, or plan an approach. This is a requirement, not a suggestion, and it applies to every session without exception — including short tasks and projects you believe you already understand.

1. Call `list_memories` first. Returns memory names only.
2. Call `read_memory` on every name that plausibly relates to the task. When unsure whether a memory is relevant, read it.
3. Only then begin the work.

You do not know what is in this project's memory until you look. Names alone tell you nothing, so `list_memories` on its own does not satisfy this step — an index you never read is an index you never used. If `initial_instructions` already handed you the memory index, you still must `read_memory` the relevant entries.

Skipping this step is the most expensive mistake you can make here. Memories exist because that context does not survive between sessions: architectural decisions, invariants that look arbitrary until explained, workarounds with reasons behind them. Work started without them re-derives what was already settled, contradicts constraints nobody told you about, and produces changes the human has to reject.

"None look relevant" is a conclusion you may reach only after reading the list, never before it. Reading the whole memory store is also wrong — select by name, then read.

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

`list_dir` returns `{ base, directories, files }`. `base` is directory you listed; every entry named relative to it. Join with `/` to get project-relative path: `base: "src/Services"` plus entry `PlayerService.luau` is `src/Services/PlayerService.luau`. `find_file` and `search_for_pattern` still answer with full project-relative paths.

Top-level symbol in result carries full name path. Nested symbol under `children` carries only own leaf name, because ancestry already spelled by chain it sits under. Join with `/` to address it: child `update` under `PlayerService` is `PlayerService/update`.

`find_referencing_symbols` returns `{ references, truncated }`, `references` keyed by file same way. Cap is `tools.max_reference_matches`, default 200; `truncated: true` means hit it, so symbol has more call sites than you see.

`find_referencing_symbols` snippet is reference line alone by default. Pass `context_lines: 1` or more when you need surrounding lines to judge how symbol used. Each extra line multiplies across every reference, so raise only when line itself not enough.

Type signatures omitted by default. Pass `include_detail: true` to `get_symbols_overview`, `find_symbol`, or `find_declaration` when you actually need signature, not just where symbol lives.

Every tool result has size ceiling, `tools.max_answer_chars`. Structured result over ceiling refused outright with message naming what to narrow — half a JSON document unreadable. Text result, such as memory, cut instead and says how much withheld.

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
