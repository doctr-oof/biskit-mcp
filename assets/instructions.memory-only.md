# Biskit Instructions Manual

Biskit running in **memory-only mode** for this project. Project memory, file listing, and text
search available. Luau language server not running, and its tools are not registered.

Read-only on source: never edit, create, delete source files. Use own native write tools for all
edits.

## Memory-only mode

These tools do **not** exist in this session. Do not attempt to call them:

`get_symbols_overview`, `find_symbol`, `find_declaration`, `find_referencing_symbols`,
`get_file_diagnostics`, `get_symbol_diagnostics`, `restart_language_server`

So: no symbol lookup, no reference search, no type diagnostics from Biskit. Use own native tools for
reading code, and own type checker or build for verifying edits. Mode set by
`project.memory_only: true` in `.biskit/settings.yml`; only the human running the project should
change it.

## Start of every session — mandatory, no exceptions

**Check project memory before you do anything else.** Before you answer a question, open a file, run
a search, or plan an approach. This is a requirement, not a suggestion, and it applies to every
session without exception — including short tasks and projects you believe you already understand.

1. Call `list_memories` first. Returns memory names only.
2. Call `read_memory` on every name that plausibly relates to the task. When unsure whether a memory
   is relevant, read it.
3. Only then begin the work.

This matters more here than anywhere else. In memory-only mode there is no language server behind
you: no symbol lookup, no reference search, no diagnostics to catch a wrong assumption after the
fact. Stored memory is the only durable project knowledge Biskit can give you, and regex search will
not recover what it holds — decisions, invariants, the reasoning behind a workaround. Start without
it and you are guessing.

Names alone tell you nothing, so `list_memories` on its own does not satisfy this step. If
`initial_instructions` already handed you the memory index, you still must `read_memory` the
relevant entries. "None look relevant" is a conclusion you may reach only after reading the list,
never before it. Reading the whole memory store is also wrong — select by name, then read.

## Choosing a tool

| Goal | Tool |
|---|---|
| Find files by name or glob | `find_file` |
| See what is in a directory | `list_dir` |
| Regex search across file contents | `search_for_pattern` |
| Recall durable project knowledge | `list_memories` then `read_memory` |

`search_for_pattern` = the only content search Biskit offers here. Symbol-aware lookup unavailable,
so a definition search is a regex search: match on `function Name`, `local Name =`, or the
declaration form the project uses.

## Writing memories

Write memory when you learn something future session would otherwise rediscover: architectural
decisions plus reasoning, non-obvious invariants, where subsystem lives, why workaround exists,
project conventions.

Do not write memory for: file contents (read it instead), transient task state, anything already in
`CLAUDE.md` or `AGENTS.md`, summary of work you just did.

Give memories meaningful names. Nest with `/` when topic has several parts, example
`Combat/HitDetection`. Cross-reference other memories with `mem:` pointer in backticks, such as
`` `mem:Combat/HitDetection` ``. `rename_memory` rewrites those pointers automatically.

Use `edit_memory` to amend existing memory, not wholesale rewrite with `create_memory`. Wholesale
rewrites lose detail that was there for reason. `create_memory` errors when name already taken;
pass `overwrite: true` only when replacing content deliberately.
