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
| See what a ModuleScript hands back, without its body | `get_module_context` |
| Find what a module requires, or what requires it | `get_require_graph` |
| Translate between a file and its place in the game | `resolve_instance_path` |
| Check a real Roblox class, member, or enum | `query_roblox_api` |
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

`find_symbol` returns `{ symbols, truncated }`. `truncated: true` means `max_matches` cut result short: narrow with `relative_path` or raise cap. Field omitted entirely when nothing was cut, so absent = complete. Same for `list_dir` and `search_for_pattern`, and `search_for_pattern` adds `note` naming what the cut left out.

`include_kinds` and `exclude_kinds` take LSP SymbolKind numbers: 1 File, 2 Module, 3 Namespace, 4 Package, 5 Class, 6 Method, 7 Property, 8 Field, 9 Constructor, 10 Enum, 11 Interface, 12 Function, 13 Variable, 14 Constant, 15 String, 16 Number, 17 Boolean, 18 Array, 19 Object, 20 Key, 21 Null, 22 EnumMember, 23 Struct, 24 Event, 25 Operator, 26 TypeParameter. Number outside 1–26 is refused rather than matching nothing.

`symbols` is keyed by file path, and each symbol under it carries no path of its own — path comes from key it sits under. `find_declaration` returns same file-keyed shape. `get_symbols_overview` returns `{ symbols, note }`; its `symbols` is bare list, since you supplied file yourself.

`list_dir` returns `{ base, directories, files }`. `base` is directory you listed; every entry named relative to it. Join with `/` to get project-relative path: `base: "src/Services"` plus entry `PlayerService.luau` is `src/Services/PlayerService.luau`. `find_file` and `search_for_pattern` still answer with full project-relative paths.

`list_dir`, `find_file`, and `search_for_pattern` all skip what the ignore set hides: `.gitignore` while `project.respect_gitignore` on, plus `project.ignored_paths`. In Roblox projects that usually means `Packages/`. Naming an ignored directory as `relative_path` searches it anyway, which is how you reach vendored code deliberately. The sourcemap-backed tools — `resolve_instance_path`, `get_require_graph`, `get_module_context` — ignore the ignore set entirely: file rojo syncs into game is part of game.

Top-level symbol in result carries full name path. Nested symbol under `children` carries only own leaf name, because ancestry already spelled by chain it sits under. Join with `/` to address it: child `update` under `PlayerService` is `PlayerService/update`.

Table members sit under owner in `children`, so `depth` controls how much of table you see. `get_symbols_overview` defaults to `depth: 1`, which is owners plus their members; raise it for tables inside tables. `find_symbol` defaults to `depth: 0` — match alone, no members. Member nests only when owner itself declared in same file; member of table declared elsewhere stays top-level.

Variable declared inside function body is not member of anything, so traversal prunes it as noise and reports count in `omitted_children`. Symbol with `omitted_children: 2` declares two locals you cannot see, not none. Pass `include_locals: true` to `get_symbols_overview` or `find_symbol` to get them, which is what makes `depth` map body of function rather than only its members. Local is still addressable by name without flag: `find_symbol` on `cachedInfo` finds it wherever it sits.

`find_referencing_symbols` returns `{ references, truncated, note }`, `references` keyed by file same way. Cap is `tools.max_reference_matches`, default 200; `truncated: true` means hit it, so symbol has more call sites than you see.

Reference carrying `resolved_by: "text"` was found by scanning declaring file, not by language server. luau-lsp types implicit `self` of colon-declared method as fresh generic instead of owner, so `self:Method()` resolves to nothing and never reaches reference list; private helper called only that way would otherwise report zero references and read as dead code. Text-found reference is matched on symbol name alone, so confirm receiver before treating one as call site. `note` appears only when answer carries one.

`find_referencing_symbols` snippet is reference line alone by default. Pass `context_lines: 1` or more when you need surrounding lines to judge how symbol used. Each extra line multiplies across every reference, so raise only when line itself not enough.

Type signatures omitted by default. Pass `include_detail: true` to `get_symbols_overview`, `find_symbol`, or `find_declaration` when you actually need signature, not just where symbol lives. `detail` is resolved signature, same source `explain_symbol` reads, so two tools agree about one symbol. Each one costs language server a request; wide answer that runs out of budget says so in `note`, and symbols past ceiling carry no `detail` at all.

Every tool result has size ceiling, `tools.max_answer_chars`. Structured result over ceiling refused outright with message naming what to narrow — half a JSON document unreadable. Text result, such as memory, cut instead and says how much withheld.

## Types, not just locations

`find_symbol` `detail` and `explain_symbol` `signature` both report what type checker inferred, so they agree. Reach for `explain_symbol` when you have one symbol and want documentation with it, or when you can only point at line and column; reach for `include_detail` when you are already listing symbols and want their types in same answer. Either way, ask before you assume type: `local part = workspace:FindFirstChild("Thing")` declares nothing and resolves to `Instance?`.

Generic that luau-lsp inferred but signature never uses is stripped from both, because implicit `self` of colon-declared method picks one up (`GetPlayerMaid<a>`) that source never wrote. Generic signature does use is kept.

Point at symbol two ways: `name_path` plus `relative_path`, or `line` plus `column`. Both 1-based, same numbers every Biskit result gives back. Use `line`/`column` for expression that is not symbol — call site, table field, diagnostic location. Pass one or other, never both.

Returns `signature` always, `documentation` only with `include_documentation: true`. Docs verbose, so opt in when you need behavior, not when you need shape.

`find_declaration` takes `name_path` or `line`/`column`, same as `explain_symbol`. `name_path` only resolves against symbols the named file itself declares, so it cannot start from a call site — point `line`/`column` at the name in the call to follow symbol into file that declares it. Result reports symbol's full range, not just its declaration line.

`get_type_definition` different question from `find_declaration`. `find_declaration` = where this value declared. `get_type_definition` = where type declared, usually `export type` in shared module. Beats grepping `export type`.

Aim `line`/`column` at type's own name, not at value: in `local config: PlayerConfig`, point at `PlayerConfig`. Pointing at `config` returns nothing, because language server answers this from type name. Value with no written annotation has no type declaration to find — use `explain_symbol` there. `name_path` naming type declaration itself answers with that declaration.

`get_inlay_hints` = cheapest type view. Positions plus short labels over line range, no bodies. Forty-line function becomes dozen strings. Use before pulling body with `include_body`. Empty `hints` with `note` = no hints there, not failure.

`get_signature_help` answers "what arguments does this take" without reading callee. Aim `line`/`column` inside parentheses of call. Empty `signatures` carries `note`; usual cause is position outside parentheses, but luau-lsp also answers with nothing at some positions genuinely inside call, among them receiver of `self:` method call. Note says so rather than asserting one cause.

`line` past end of file and `column` past end of line are both refused, with the real length in message. Nothing is silently clamped, so a position that answers is a position that was in range.

## Roblox, not just Luau

This project is a game, not a folder of scripts. Where a file lands in the DataModel decides whether its code runs on server, client, or both, and no type checker will tell you when you got that wrong.

`get_module_context` is the call to make when you open unfamiliar module. One call returns instance path, `class_name`, owning service, `role` (`server`, `client`, `shared`, `unknown`), direct requires, direct dependents, public surface, diagnostic counts. Replaces four to six separate calls. Start here, then narrow.

`role` reads `class_name` first and service only after it, because `Script` runs on server and `LocalScript` on client wherever they sit. `ModuleScript` takes role from its service. Rojo sourcemaps carry no RunContext, so `Script` placed for client RunContext still reads as `server` — `class_name` is in the answer, so check it when that matters.

Its `api` section is what the ModuleScript hands back: members of returned table with resolved signatures, plus `export type` declarations. No body. Use it instead of reading module you only intend to call. `api.return_kind` says what shape came back — `table`, `function`, `table_literal`, `expression`, `conditional`, `unknown`, `none`.

`none` = Script or LocalScript, no public surface, and only ever reported when sourcemap agrees file is not ModuleScript. `conditional` = module returns from more than one branch and branches disagree on shape. Branches that agree keep specific kind — module returning function from both arms of top-level `if` is `function`, not `conditional`. Either way `api.note` names each branch and its line, so surface depending on condition is always visible. `unknown` = sourcemap calls it ModuleScript but return could not be located statically; read end of file. Returns inside top-level `if`/`else`, `do`, or loop count as module's own — return inside function body does not. Luau `if ... then ... else` expression is value, not block, and does not hide returns that follow it. `api.note` explains every case where surface is empty or partial.

`get_require_graph` sees module-level coupling that `find_referencing_symbols` cannot: that tool sees symbol references, not requires. Pass `relative_path` for one module, `direction` (`dependencies`, `dependents`, `both`), `depth` for transitive hops. Omit `relative_path` for project-wide answer naming every require cycle.

Graph scans the project walk plus every Luau file the sourcemap names, so gitignored vendored trees such as `Packages/` are in it and requires into them resolve.

Requires resolved through sourcemap, so `script.Parent.Parent.Shared.X`, `game:GetService("ReplicatedStorage").Y`, `:WaitForChild("Z")`, and `@Alias/Module` all resolve. Requires that cannot be resolved statically — `require(modules[name])`, require through wrapper function — land in `unresolved` with reason. Read that list. Empty `dependencies` plus non-empty `unresolved` means module has real dependencies Biskit cannot see, not that it has none. Project using runtime module loader instead of `require` has empty graph and that is honest, not broken.

`shared("Foo")` is a require too. Sawhorse frameworks give the `shared` global a `__call` metamethod, so it requires the module whose file is named `Foo.luau`. The language server resolves it as `require`, and so does the graph: those calls are ordinary edges in `dependencies` and `dependents`. Bare stem or partial path (`shared("Jobs/Runner")`), case-insensitive, `dir/init.luau` addressed as `dir`. Where several files carry the name, Biskit applies its own heuristic, not the loader's: candidate sharing most instance-tree ancestors with caller wins, and shallowest candidate breaks remaining tie. Frameworks decide collisions by index order instead, so check the answer when name is genuinely duplicated. Candidates never include anything under `_Index` — package manager writes second copy of every vendored module there and no loader addresses those by name. Tie that survives both rules lands in `unresolved` naming every candidate, so pick one and write the partial path. `shared(name)` and `shared("a" .. b)` land in `unresolved` — the language server does not resolve those either, so the diagnostic on that line is real. `shared.someField` is table access, not a require. If `get_status` reports `shared_require.graph_edges` false, the graph is not counting these calls and its dependency lists understate what the module actually needs.

`resolve_instance_path` goes both ways. Pass `instance_path` for file behind `game.ReplicatedStorage.Shared.Combat`; pass `relative_path` for where `src/Shared/Combat/init.luau` ends up in game. Never guess this translation. File that resolves to nothing is not synced by rojo project, so editing it changes nothing at runtime.

Every DataModel answer carries `sourcemap` with mtime and age. Old sourcemap describes game that no longer exists. Check it before trusting instance path that surprises you.

`query_roblox_api` is ground truth for Roblox API, read from same type definitions the checker uses. Do not recall Roblox API from memory — hallucinated method looks exactly like real one until it runs.

Ask it for class (`BasePart`), member (`TweenService:Create`, `BasePart.Anchored`), or enum (`Enum.EasingStyle`). Member answer carries signature, parameter docs, return docs, deprecation plus replacement. Class answer lists own members only; pass `include_inherited: true` to walk ancestry, `member_filter` to narrow. Three counts, each answering different question: `returned_count` is how many members are in answer, `matched_count` how many survived `member_filter` before `max_members` capped list, `total_member_count` how many class carried before filter. Last two omitted when they equal the one above. Capped answer is sorted first: own members before inherited, current before deprecated, so sample is worth reading. Members hidden at current `lsp.roblox_security_level` are absent, and answer names the level it read.

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

`start_line`/`end_line` are 1-based and inclusive. `0`, or `start_line` past `end_line`, is refused rather than answered empty.

## When the language server misbehaves

Empty result has three causes and empty result shows none of them: project genuinely lacks symbol, sourcemap missing or stale, language server dead. Call `get_status` to tell them apart before you retry.

`get_status` reports project root and how it was chosen, language server state, sourcemap freshness against newest Luau file, memory count, every setting that differs from default. `stale: true` on sourcemap means script added or moved since it was generated, so instance paths and DataModel types are wrong until it is regenerated.

If symbol tools return empty results for file you know has symbols, or diagnostics look stale against file you just changed, call `restart_language_server`. Cheap. Do not restart reflexively for empty result that means only "no matches" — verify with `get_symbols_overview` or `get_status` first.
