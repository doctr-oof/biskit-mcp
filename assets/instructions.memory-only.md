# Biskit MCP Instructions (memory-only mode)

<Main name="BiskitInstructionsManual">
<Section name="WhatBiskitIs" desc="Identity and hard boundary. Read first.">
    Biskit running in memory-only mode for this project. Project memory, file listing, and text
    search available. Luau language server not running, and its tools are not registered.

    <Rule>Use own native write tools for all edits.</Rule>
</Section>

<Section name="MemoryOnlyMode" desc="What this mode removes. All restrictions MANDATORY.">
    <Rule>
        These tools do not exist in this session. Do not attempt to call them:
        `get_symbols_overview`, `find_symbol`, `find_declaration`, `find_referencing_symbols`,
        `get_file_diagnostics`, `get_symbol_diagnostics`, `restart_language_server`,
        `explain_symbol`, `get_type_definition`, `get_inlay_hints`, `get_signature_help`,
        `resolve_instance_path`, `get_require_graph`, `get_module_context`, `query_roblox_api`
    </Rule>

    <Rule>
        So: no symbol lookup, no reference search, no type diagnostics from Biskit, and nothing
        about the Roblox DataModel or the Roblox API. Use own native tools for reading code, and
        own type checker or build for verifying edits.
    </Rule>

    <Rule>
        Mode set by `project.memory_only: true` in `.biskit/settings.yml`; only the human running the
        project should change it.
    </Rule>
</Section>

<Section name="SessionStart" desc="Mandatory, no exceptions. Runs before any other work.">
    <Behavior name="CheckMemoryFirst">
        Check project memory before you do anything else: before you answer a question, open a
        file, run a search, or plan an approach. Every session, including short tasks and projects
        you believe you already understand. In this mode stored memory is the only durable project
        knowledge Biskit has, and regex search will not recover what it holds.

        1. Read the memory index in `AvailableMemories` below. It is names only.
        2. Call `read_memory` on every name that plausibly relates to the task. When unsure whether a
           memory is relevant, read it.
        3. Only then begin the work.
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
        `search_for_pattern` = the only content search Biskit offers here. A definition search is a
        regex search: match on `function Name`, `local Name =`, or the declaration form the project
        uses.
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
        Wholesale rewrites lose detail that was there for reason.
    </Rule>
</Section>
</Main>
