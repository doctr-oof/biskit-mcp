# Biskit MCP Instructions (memory-only mode)

<Main name="BiskitInstructionsManual">
<Section name="WhatBiskitIs" desc="Identity and hard boundary. Read first.">
    Biskit running in memory-only mode for this project. Project memory, file listing, and text
    search available. Luau language server not running, and its tools are not registered.

    <Rule>Use own native write tools for all edits.</Rule>
</Section>

<Section name="MemoryOnlyMode" desc="What this mode removes, and why. All restrictions MANDATORY.">
    <Rule>
        These tools do not exist in this session. Do not attempt to call them:
        `get_symbols_overview`, `find_symbol`, `find_declaration`, `find_referencing_symbols`,
        `get_file_diagnostics`, `get_symbol_diagnostics`, `restart_language_server`,
        `explain_symbol`, `get_type_definition`, `get_inlay_hints`, `get_signature_help`,
        `resolve_instance_path`, `get_require_graph`, `get_module_context`, `query_roblox_api`
    </Rule>

    <Rule>
        So: no symbol lookup, no reference search, no type diagnostics from Biskit, and nothing about
        the Roblox DataModel or the Roblox API, because the sourcemap and the type definitions those
        answers are read from are loaded for the language server that is not running.
    </Rule>

    <Rule>
        Use own native tools for reading code, and own type checker or build for verifying edits.
    </Rule>

    <Rule>
        Mode set by `project.memory_only: true` in `.biskit/settings.yml`; only the human running the
        project should change it.
    </Rule>
</Section>

<Section name="SessionStart" desc="Mandatory, no exceptions. Runs before any other work.">
    <Behavior name="CheckMemoryFirst">
        Check project memory before you do anything else. Before you answer a question, open a file,
        run a search, or plan an approach. This is a requirement, not a suggestion, and it applies to
        every session without exception — including short tasks and projects you believe you already
        understand.

        1. Read the memory index in `AvailableMemories` below. It is names only.
        2. Call `read_memory` on every name that plausibly relates to the task. When unsure whether a
           memory is relevant, read it.
        3. Only then begin the work.

        You do not know what is in this project's memory until you look.
    </Behavior>

    <Behavior name="WhyItMattersMoreHere">
        This matters more here than anywhere else. In memory-only mode there is no language server
        behind you: no symbol lookup, no reference search, no diagnostics to catch a wrong assumption
        after the fact. Stored memory is the only durable project knowledge Biskit can give you, and
        regex search will not recover what it holds — decisions, invariants, the reasoning behind a
        workaround. Start without it and you are guessing.
    </Behavior>

    <Rule>"None look relevant" is a conclusion you may reach only after reading the list, never before it.</Rule>
    <Rule>Reading the whole memory store is also wrong — select by name, then read.</Rule>
</Section>

<Section name="ChoosingATool" desc="Goal to tool mapping for the tools this mode registers.">
    <Tool name="find_file" use="Find files by name or glob" />
    <Tool name="list_dir" use="See what is in a directory" />
    <Tool name="search_for_pattern" use="Regex search across file contents" />
    <Tool name="list_memories" use="Recall durable project knowledge, then `read_memory`" />
    <Tool name="get_status" use="Confirm which project Biskit is serving, and in which mode" />

    <Rule>
        `search_for_pattern` = the only content search Biskit offers here. Symbol-aware lookup
        unavailable, so a definition search is a regex search: match on `function Name`, `local Name
        =`, or the declaration form the project uses.
    </Rule>

    <Rule>
        All three file tools skip what the ignore set hides: `.gitignore` while
        `project.respect_gitignore` is on, plus `project.ignored_paths`. Naming an ignored directory
        as `relative_path` searches it anyway.
    </Rule>

    <Rule>
        A cut answer sets `truncated` and carries a `note` naming what was left out; both fields are
        absent when nothing was cut, so absent means complete.
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
        Wholesale rewrites lose detail that was there for reason. `create_memory` errors when name
        already taken; pass `overwrite: true` only when replacing content deliberately.
    </Rule>

    <Rule>
        `edit_memory` replacement expands `$1` and `${name}` as capture groups, so dollar sign meant
        literally must be written `$$`: `costs $$5`, not `costs $5`. Replacement naming group pattern
        does not define is refused, not silently emptied.
    </Rule>
</Section>
</Main>
