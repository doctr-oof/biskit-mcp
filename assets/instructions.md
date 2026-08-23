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
| Learn what a symbol's type actually resolves to | `explain_symbol` |
| Jump to where a *type* is declared | `get_type_definition` |
| See inferred types over a line range, cheaply | `get_inlay_hints` |
| Learn the arguments of a call you are writing | `get_signature_help` |
| Find every caller or user of a symbol | `find_referencing_symbols` |
| Orient on a module you have not seen before | `get_module_context` |
| See what a ModuleScript hands back, without its body | `get_module_api` |
| Find what a module requires, or what requires it | `get_require_graph` |
| Translate between a file and its place in the game | `resolve_instance_path` |
| Check a real Roblox class, member, or enum | `query_roblox_api` |
| Rename a symbol across the project | `plan_symbol_rename` |
| Check whether a file type-checks | `get_file_diagnostics` |
| Check a symbol and its callers for breakage after an edit | `get_symbol_diagnostics` |
| Find files by name or glob | `find_file` |
| See what is in a directory | `list_dir` |
| Regex search across file contents | `search_for_pattern` |
| Work out why a tool returned nothing | `get_status` |

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

Table members sit under owner in `children`, so `depth` controls how much of table you see. `get_symbols_overview` defaults to `depth: 1`, which is owners plus their members; raise it for tables inside tables. `find_symbol` defaults to `depth: 0` — match alone, no members. Member nests only when owner itself declared in same file; member of table declared elsewhere stays top-level.

`find_referencing_symbols` returns `{ references, truncated }`, `references` keyed by file same way. Cap is `tools.max_reference_matches`, default 200; `truncated: true` means hit it, so symbol has more call sites than you see.

`find_referencing_symbols` snippet is reference line alone by default. Pass `context_lines: 1` or more when you need surrounding lines to judge how symbol used. Each extra line multiplies across every reference, so raise only when line itself not enough.

Type signatures omitted by default. Pass `include_detail: true` to `get_symbols_overview`, `find_symbol`, or `find_declaration` when you actually need signature, not just where symbol lives.

Every tool result has size ceiling, `tools.max_answer_chars`. Structured result over ceiling refused outright with message naming what to narrow — half a JSON document unreadable. Text result, such as memory, cut instead and says how much withheld.

## Types, not just locations

`find_symbol` `detail` is declared shape. `explain_symbol` is what type checker actually inferred. Different answers whenever type not written out: `local part = workspace:FindFirstChild("Thing")` declares nothing, resolves to `Instance?`. Ask `explain_symbol` before you assume a type.

Point at symbol two ways: `name_path` plus `relative_path`, or `line` plus `column`. Both 1-based, same numbers every Biskit result gives back. Use `line`/`column` for expression that is not symbol — call site, table field, diagnostic location. Pass one or other, never both.

Returns `signature` always, `documentation` only with `include_documentation: true`. Docs verbose, so opt in when you need behavior, not when you need shape.

`get_type_definition` different question from `find_declaration`. `find_declaration` = where this value declared. `get_type_definition` = where type declared, usually `export type` in shared module. Beats grepping `export type`.

Aim it at type's own name, not at value: in `local config: PlayerConfig`, point `line`/`column` at `PlayerConfig`. Pointing at `config` returns nothing, because language server answers this from type name. Value with no written annotation has no type declaration to find — use `explain_symbol` there.

`get_inlay_hints` = cheapest type view. Positions plus short labels over line range, no bodies. Forty-line function becomes dozen strings. Use before pulling body with `include_body`. Empty `hints` with `note` = no hints there, not failure.

`get_signature_help` answers "what arguments does this take" without reading callee. Aim `line`/`column` inside parentheses of call. Aimed at declaration instead returns nothing, and says so in `note`.

## Roblox, not just Luau

This project is a game, not a folder of scripts. Where a file lands in the DataModel decides whether its code runs on server, client, or both, and no type checker will tell you when you got that wrong.

`get_module_context` is the call to make when you open unfamiliar module. One call returns instance path, owning service, `role` (`server`, `client`, `shared`, `unknown`), direct requires, direct dependents, public surface, diagnostic counts. Replaces four to six separate calls. Start here, then narrow.

`get_module_api` returns only what ModuleScript hands back: members of returned table with types, plus `export type` declarations. No body. Use before reading module you only intend to call. `return_kind` says what shape came back — `table`, `function`, `table_literal`, `expression`, `none`. `none` = Script or LocalScript, no public surface. `note` explains every case where surface is empty or partial.

`get_require_graph` sees module-level coupling that `find_referencing_symbols` cannot: that tool sees symbol references, not requires. Pass `relative_path` for one module, `direction` (`dependencies`, `dependents`, `both`), `depth` for transitive hops. Omit `relative_path` for project-wide answer naming every require cycle.

Requires resolved through sourcemap, so `script.Parent.Parent.Shared.X`, `game:GetService("ReplicatedStorage").Y`, `:WaitForChild("Z")`, and `@Alias/Module` all resolve. Requires that cannot be resolved statically — `require(modules[name])`, require through wrapper function — land in `unresolved` with reason. Read that list. Empty `dependencies` plus non-empty `unresolved` means module has real dependencies Biskit cannot see, not that it has none. Project using runtime module loader instead of `require` has empty graph and that is honest, not broken.

`resolve_instance_path` goes both ways. Pass `instance_path` for file behind `game.ReplicatedStorage.Shared.Combat`; pass `relative_path` for where `src/Shared/Combat/init.luau` ends up in game. Never guess this translation. File that resolves to nothing is not synced by rojo project, so editing it changes nothing at runtime.

Every DataModel answer carries `sourcemap` with mtime and age. Old sourcemap describes game that no longer exists. Check it before trusting instance path that surprises you.

`query_roblox_api` is ground truth for Roblox API, read from same type definitions the checker uses. Do not recall Roblox API from memory — hallucinated method looks exactly like real one until it runs.

Ask it for class (`BasePart`), member (`TweenService:Create`, `BasePart.Anchored`), or enum (`Enum.EasingStyle`). Member answer carries signature, parameter docs, return docs, deprecation plus replacement. Class answer lists own members only; pass `include_inherited: true` to walk ancestry, `member_filter` to narrow. Members hidden at current `lsp.roblox_security_level` are absent, and answer names the level it read.

## Renaming a symbol

`plan_symbol_rename` returns edits, applies none. Biskit never writes source; you apply plan with own edit tools.

Result is `edits` keyed by file, each entry `line`, `column`, `end_line`, `end_column`, `old_text`, `new_text`, sorted by position. Apply each file's edits bottom upwards so earlier positions stay valid.

Use this instead of search and replace. Grep rename hits same-named symbol in unrelated module and misses call site written differently; language server hits exactly binding you named.

`note` plus `references` in result and empty `edits` means server produced no plan. Those references are every use it sees, not rename plan — verify each before touching it.

## After you edit code

Biskit read source from disk each request, so edits visible immediately. After non-trivial edit, call `get_file_diagnostics` on changed file. If edit changed symbol signature or behavior, call `get_symbol_diagnostics` with `check_symbol_references: true` to catch breakage at call sites.

## Writing memories

Write memory when you learn something future session would otherwise rediscover: architectural decisions plus reasoning, non-obvious invariants, where subsystem lives, why workaround exists, project conventions.

Do not write memory for: file contents (read it instead), transient task state, anything already in `CLAUDE.md` or `AGENTS.md`, summary of work you just did.

Give memories meaningful names. Nest with `/` when topic has several parts, example `Combat/HitDetection`. Cross-reference other memories with `mem:` pointer in backticks, such as `` `mem:Combat/HitDetection` ``. `rename_memory` rewrites those pointers automatically.

Use `edit_memory` to amend existing memory, not wholesale rewrite with `create_memory`. Wholesale rewrites lose detail that was there for reason. `create_memory` errors when name already taken; pass `overwrite: true` only when replacing content deliberately.

`edit_memory` replacement expands `$1` and `${name}` as capture groups, so dollar sign meant literally must be written `$$`: `costs $$5`, not `costs $5`. Replacement naming group pattern does not define is refused, not silently emptied.

## Diagnostics severity

`min_severity` filters results: `1` errors only, `2` errors and warnings, `3` adds information, `4` adds hints. Default `2`. Ask `1` when you care only whether something broken.

Diagnostics grouped file, then severity, then `symbols` keyed by name path. Diagnostics belonging to no symbol land in `unscoped` list beside it.

## When the language server misbehaves

Empty result has three causes and empty result shows none of them: project genuinely lacks symbol, sourcemap missing or stale, language server dead. Call `get_status` to tell them apart before you retry.

`get_status` reports project root and how it was chosen, language server state, sourcemap freshness against newest Luau file, memory count, every setting that differs from default. `stale: true` on sourcemap means script added or moved since it was generated, so instance paths and DataModel types are wrong until it is regenerated.

If symbol tools return empty results for file you know has symbols, or diagnostics look stale against file you just changed, call `restart_language_server`. Cheap. Do not restart reflexively for empty result that means only "no matches" — verify with `get_symbols_overview` or `get_status` first.
