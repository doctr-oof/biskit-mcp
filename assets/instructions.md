# Biskit MCP Instructions

<Main name="BiskitInstructionsManual">
<Section name="WhatBiskitIs" desc="Identity and hard boundary. Read first.">
    Biskit = symbolic code-intelligence and project-memory server for this Luau project. Symbol
    lookup, references, and diagnostics come from the real language server, not text search.
    Memories are durable curated notes that survive between sessions.

    <Rule>Use own native write tools for all edits.</Rule>
</Section>

<Section name="SessionStart" desc="Mandatory, no exceptions. Runs before any other work.">
    <Behavior name="CheckMemoryFirst">
        Check project memory before you do anything else: before you answer a question, open a
        file, run a search, or plan an approach. Every session, including short tasks and projects
        you believe you already understand.

        1. Read the memory index in `AvailableMemories` below. It is names only.
        2. Call `read_memory` on every name that plausibly relates to the task. When unsure whether a
           memory is relevant, read it.
        3. Only then begin the work.
    </Behavior>

    <Rule>"None look relevant" is a conclusion you may reach only after reading the list, never before it.</Rule>
    <Rule>Reading the whole memory store is also wrong — select by name, then read.</Rule>
</Section>

<Section name="ChoosingATool" desc="Goal to tool mapping, plus when a tool is the wrong reach.">
    <Tool name="get_symbols_overview" use="Understand a file's structure before reading it" />
    <Tool name="find_symbol" use="Read one symbol's implementation, with `include_body: true`" />
    <Tool name="find_symbol" use="Locate a symbol anywhere in the project" />
    <Tool name="find_declaration" use="Jump to where a symbol is defined" />
    <Tool name="explain_symbol" use="Learn what a symbol's type actually resolves to" />
    <Tool name="get_type_definition" use="Jump to where a *type* is declared" />
    <Tool name="get_inlay_hints" use="See inferred types over a line range, cheaply" />
    <Tool name="get_signature_help" use="Learn the arguments of a call you are writing" />
    <Tool name="find_referencing_symbols" use="Find every caller or user of a symbol" />
    <Tool name="get_module_context" use="Orient on a module you have not seen before" />
    <Tool name="get_module_context" use="See what a ModuleScript hands back, without its body" />
    <Tool name="get_require_graph" use="Find what a module requires, or what requires it" />
    <Tool name="resolve_instance_path" use="Translate between a file and its place in the game" />
    <Tool name="query_roblox_api" use="Check a real Roblox class, member, or enum" />
    <Tool name="get_file_diagnostics" use="Check whether a file type-checks" />
    <Tool name="get_symbol_diagnostics" use="Check a symbol and its callers for breakage after an edit" />
    <Tool name="list_wally_packages" use="See which Wally packages this project depends on" />
    <Tool name="search_wally_packages" use="Find a package in the Wally registry before adding it" />
    <Tool name="add_wally_package" use="Declare a Wally dependency and install it" />
    <Tool name="remove_wally_package" use="Drop a Wally dependency and rebuild the package tree" />
    <Tool name="find_file" use="Find files by name or glob" />
    <Tool name="list_dir" use="See what is in a directory" />
    <Tool name="search_for_pattern" use="Regex search across file contents" />
    <Tool name="read_memory" use="Read a memory named in the index below" />
    <Tool name="list_memories" use="Re-list memories after writing one; the index below is the session's starting list" />
    <Tool name="get_status" use="Work out why a tool returned nothing" />

    <Rule>
        Prefer symbolic tools over whole files. `get_symbols_overview` then targeted `find_symbol`
        almost always cheaper than reading a module for one function.
    </Rule>
</Section>

<Section name="NamePaths" desc="The shape of what symbol tools give back.">
    <Topic name="DuplicateNames">
        When a file has two symbols of the same name, `get_symbols_overview` labels them
        `UserInfo[0]` and `UserInfo[1]`. Bare `UserInfo` matches both; tools that take a single
        symbol error on it, so pass the indexed label back verbatim there.
    </Topic>

    <Topic name="Truncation">
        `truncated: true` on `find_symbol`, `find_referencing_symbols`, `list_dir`, or
        `search_for_pattern` means a cap cut the answer short: narrow with `relative_path` or raise
        the cap. The field is omitted when nothing was cut, so absent = complete. A `note` names
        what the cut left out where the tool carries one.
    </Topic>

    <Topic name="ResultShape">
        `symbols` and `references` are keyed by file path; each entry under a key carries no path
        of its own. `find_declaration` uses the same file-keyed shape. `get_symbols_overview`
        returns `{ symbols, note }` with a bare list, since you supplied the file.

        A top-level symbol carries its full name path. A nested symbol under `children` carries
        only its leaf name; join with `/` to address it: child `update` under `PlayerService` is
        `PlayerService/update`.

        `omitted_children: 2` = two locals hidden, not none; `include_locals: true` shows them.
    </Topic>

    <Topic name="DirectoryListings">
        `list_dir` returns `{ base, directories, files }`, every entry relative to `base`. Join with
        `/` for the project-relative path. `find_file` and `search_for_pattern` answer with full
        project-relative paths.
    </Topic>

    <Topic name="IgnoreSet">
        The sourcemap-backed tools — `resolve_instance_path`, `get_require_graph`,
        `get_module_context` — ignore the ignore set entirely: a file rojo syncs into the game is
        part of the game, `Packages/` included.
    </Topic>

    <Topic name="References">
        A reference carrying `resolved_by: "text"` was found by scanning the declaring file, not by
        the language server. luau-lsp types the implicit `self` of a colon-declared method as a
        fresh generic, so `self:Method()` never reaches the language server's list; the text scan
        keeps a helper called only that way from reading as dead code. It is matched on name
        alone, so confirm the receiver before treating one as a call site.
    </Topic>

    <Topic name="AnswerSizeCeiling">
        Every result has a size ceiling, `tools.max_answer_chars`. A structured result over it is
        refused with a message naming what to narrow; a text result, such as a memory, is cut and
        says how much was withheld.
    </Topic>
</Section>

<Section name="TypesNotJustLocations" desc="Reading inferred types, declarations, and signatures.">
    <Topic name="DetailVersusExplain">
        `find_symbol` `detail` and `explain_symbol` `signature` report the same inferred type.
        Reach for `explain_symbol` for one symbol with its documentation, or when you can only
        point at a line and column; reach for `include_detail` when already listing symbols. Ask
        before you assume a type: `workspace:FindFirstChild("Thing")` resolves to `Instance?`.

        A generic luau-lsp inferred but the signature never uses is stripped from both, because the
        implicit `self` of a colon-declared method picks one up (`GetPlayerMaid<a>`) that source
        never wrote.
    </Topic>

    <Topic name="ExplainSymbol">
        `name_path` in the answer names the symbol at the position, resolved through the same
        lookup `find_declaration` makes. `declared_in` is set when that declaration lives in
        another file. `containing_symbol` names the symbol the position sits inside, a different
        question. A declaration the language server cannot place inside the project leaves
        `name_path` absent and says so in `note`.
    </Topic>

    <Topic name="FindDeclaration">
        Result reports the symbol's full range, not just its declaration line.
    </Topic>

    <Topic name="GetTypeDefinition">
        `find_declaration` = where this value is declared. `get_type_definition` = where its type
        is declared. A value with no written annotation has no type declaration to find; use
        `explain_symbol` there.
    </Topic>

    <Topic name="GetInlayHints">
        Use before pulling a body with `include_body`. Empty `hints` with `note` = no hints there,
        not failure.
    </Topic>

    <Topic name="GetSignatureHelp">
        Empty `signatures` carries a `note`. Usual cause is a position outside the parentheses, but
        luau-lsp also answers nothing at some positions genuinely inside a call, among them the
        receiver of a `self:` method call. For a variadic callee the parameter names in `label` are
        your own arguments, not the callee's; read its real signature with `explain_symbol` or
        `query_roblox_api` on the callee itself. `active_parameter` still tracks correctly.
    </Topic>
</Section>

<Section name="RobloxNotJustLuau" desc="Where a file lands in the DataModel decides where its code runs.">
    <Topic name="GetModuleContext">
        Start here on an unfamiliar module, then narrow. One call returns instance path,
        `class_name`, owning service, `role` (`server`, `client`, `shared`, `unknown`), direct
        requires, direct dependents, public surface, diagnostic counts.

        `role` reads `class_name` first and service after: `Script` runs on server and
        `LocalScript` on client wherever they sit; `ModuleScript` takes role from its service. Rojo
        sourcemaps carry no RunContext, so a `Script` placed for client RunContext still reads as
        `server`; check `class_name` when that matters.

        `api` is what the ModuleScript hands back: members of the returned table with resolved
        signatures, plus `export type` declarations, no body. Use it instead of reading a module
        you only intend to call. `api.return_kind` is `table`, `function`, `table_literal`,
        `expression`, `conditional`, `unknown`, or `none`. `none` = Script or LocalScript, no
        public surface. `conditional` = branches disagree on shape; branches that agree keep the
        specific kind. `unknown` = ModuleScript whose return could not be located statically; read
        the end of the file. Returns inside a top-level `if`/`else`, `do`, or loop count as the
        module's own; a return inside a function body does not. `api.note` explains every empty or
        partial surface, naming each branch and its line.
    </Topic>

    <Topic name="GetRequireGraph">
        The graph covers the project walk plus every Luau file the sourcemap names, so gitignored
        vendored trees such as `Packages/` are in it and requires into them resolve.
        `script.Parent.Parent.Shared.X`, `game:GetService("ReplicatedStorage").Y`,
        `:WaitForChild("Z")`, and `@Alias/Module` all resolve. Requires that cannot be resolved
        statically — `require(modules[name])`, require through a wrapper function — land in
        `unresolved` with a reason. Read that list: empty `dependencies` plus non-empty `unresolved`
        means real dependencies Biskit cannot see, not none. A project using a runtime module
        loader instead of `require` has an empty graph, and that is honest.
    </Topic>

    <Topic name="SharedRequire">
        `shared("Foo")` is a require too, resolved by the language server and the graph as an
        ordinary edge to the module whose file is named `Foo.luau`. Bare stem or partial path
        (`shared("Jobs/Runner")`), case-insensitive, `dir/init.luau` addressed as `dir`. Where
        several files carry the name, Biskit picks the candidate sharing the most instance-tree
        ancestors with the caller, shallowest breaking the tie; frameworks decide by index order
        instead, so check the answer when a name is genuinely duplicated. Nothing under `_Index` is
        a candidate. A tie that survives lands in `unresolved` naming every candidate; write the
        partial path. `shared(name)` and `shared("a" .. b)` land in `unresolved`, and the
        diagnostic on that line is real. `shared.someField` is table access, not a require. If
        `get_status` reports `shared_require.graph_edges` false, dependency lists understate what
        the module needs.
    </Topic>

    <Topic name="ResolveInstancePath">
        A file that resolves to nothing is not synced by the rojo project, so editing it changes
        nothing at runtime.
    </Topic>

    <Topic name="SourcemapStaleness">
        Every DataModel answer carries `sourcemap` with mtime, age, and `stale`. `stale: true`
        means a Luau file is newer than the sourcemap, so a script added or moved since is
        invisible; regenerate before trusting it. Age alone does not answer this. `get_status`
        names the offending file in `sourcemap.newest_source`.
    </Topic>

    <Topic name="QueryRobloxApi">
        A member answer carries signature, parameter docs, return docs, deprecation plus
        replacement. A class answer carries three counts: `returned_count` is how many members are
        in the answer, `matched_count` how many survived `member_filter` before `max_members`
        capped it, `total_member_count` how many the class carried before the filter; the last
        two are omitted when equal to the one above. Members hidden at the current
        `lsp.roblox_security_level` are absent, and the answer names the level it read.

        An enum answer lists item names and, where documentation carries one, `learn_more_link`.
        It does not carry numeric `EnumItem.Value`; follow `learn_more_link` or call
        `Enum.X:FromValue(n)` at runtime.
    </Topic>
</Section>

<Section name="WallyPackages" desc="The four tools that touch dependencies. Only reach for them when asked.">
    <Rule>
        Do not survey Wally packages on your own initiative. Use these tools when the human asks
        about dependencies, names a package, or asks for one to be added or removed. Every registry
        call costs a network round trip.
    </Rule>

    <Rule>
        Biskit never installs Wally. When no `wally` executable is on PATH, all four tools fail
        saying so, and that is the whole answer: do not offer to install it, and do not edit
        `wally.toml` with your own write tools instead. A version manager shim (Aftman, Rokit) can
        be present but refuse to run; the error says which.
    </Rule>

    <Topic name="ListWallyPackages">
        `installed: true` means the package's own folder is under `_Index` and a require of it
        resolves; `installed: false` means it is missing or the index was left half written, so
        every require of it fails whatever `wally.toml` says. `lock_out_of_date: true` means the
        lockfile holds a version the requirement no longer allows. `alias` is the name the package
        is required by (`Packages.Roact`, not `Packages.roact`); read it there rather than guessing.
    </Topic>

    <Topic name="SearchWallyPackages">
        `latest_version` is what `add_wally_package` pins when passed no version. Answers are
        cached briefly and requests are paced, so a repeated query is free and a burst of new ones
        is slow on purpose.
    </Topic>

    <Topic name="AddAndRemove">
        Both tools report `install_ran`, which says only whether `wally install` was run, not what
        is on disk; `list_wally_packages` and its `installed` field answer that.

        A failed `wally install` leaves `Packages`, `ServerPackages`, and `DevPackages` empty,
        packages unrelated to the call included. Biskit rolls the `wally.toml` edit back, but
        restoring the tree takes a successful install. Report it to the human; do not continue.

        Before removing, call `get_require_graph` or `find_referencing_symbols` on what requires
        the package; the diagnostic arrives only after the tree is rebuilt.
    </Topic>

    <Topic name="AfterInstalling">
        Installing rewrites the package tree, so the sourcemap from before it is stale and the
        language server will not resolve requires into the new package. Regenerate it.
    </Topic>
</Section>

<Section name="AfterYouEditCode" desc="Verification loop once your own write tools have run.">
    <Rule>
        Biskit read source from disk each request, so edits visible immediately.
    </Rule>
    <Rule>
        After non-trivial edit, call `get_file_diagnostics` on changed file.
    </Rule>
    <Rule>
        If edit changed symbol signature or behavior, call `get_symbol_diagnostics` with
        `check_symbol_references: true` to catch breakage at call sites.
    </Rule>
</Section>

<Section name="WritingMemories" desc="What earns a memory, what does not, and how to name and amend one.">
    <Rule>
        Write memory when you learn something future session would otherwise rediscover:
        architectural decisions plus reasoning, non-obvious invariants, where subsystem lives, why
        workaround exists, project conventions.
    </Rule>

    <Rule>
        Do not write memory for: file contents (read it instead), transient task state, anything
        already in `CLAUDE.md` or `AGENTS.md`, summary of work you just did.
    </Rule>

    <Rule>
        Give memories meaningful names. Nest with `/` when topic has several parts, example
        `Combat/HitDetection`. Cross-reference other memories with `mem:` pointer in backticks, such
        as `` `mem:Combat/HitDetection` ``. `rename_memory` rewrites those pointers automatically.
    </Rule>

    <Rule>
        Use `edit_memory` to amend existing memory, not wholesale rewrite with `create_memory`.
        Wholesale rewrites lose detail that was there for reason.
    </Rule>
</Section>

<Section name="DiagnosticsSeverity" desc="Reading diagnostic answers.">
    <Rule>
        Diagnostics grouped file, then severity, then `symbols` keyed by name path. Diagnostics
        belonging to no symbol land in `unscoped` list beside it.
    </Rule>
</Section>

<Section name="WhenTheLanguageServerMisbehaves" desc="Diagnosing an empty answer before you retry.">
    <Rule>
        Empty result has three causes and shows none of them: project genuinely lacks symbol,
        sourcemap missing or stale, language server dead. Call `get_status` to tell them apart
        before you retry.
    </Rule>

    <Rule>
        If symbol tools return empty for file you know has symbols, or diagnostics look stale
        against file you just changed, call `restart_language_server`. Do not restart reflexively
        for empty result that means only "no matches" — verify with `get_symbols_overview` or
        `get_status` first.
    </Rule>
</Section>
</Main>
