# initial_instructions

Returns Biskit's usage manual, unmodified. No memory index is appended; the manual itself tells the agent to call `list_memories` and then `read_memory` on relevant entries.

## Parameters
None. The handler takes `Parameters(NoArguments {})`.

## Implementation
- Handler: `Biskit::initial_instructions` in `src/server/mod.rs`. It returns `self.text(prompts::instructions_manual(self.inner.settings.project.memory_only))`.
- `prompts::instructions_manual(memory_only)` in `src/prompts.rs` picks the manual: `INSTRUCTIONS_MANUAL` (`assets/instructions.md`) normally, `MEMORY_ONLY_INSTRUCTIONS_MANUAL` (`assets/instructions.memory-only.md`) when `project.memory_only` is true. Both are `include_str!`ed, so editing either asset requires a rebuild.
- Tool description comes from the `description!(initial_instructions)` arm in `src/server/descriptions.rs`.
- Tests in `src/prompts.rs` and `src/server/mod.rs` assert the manual has no `AvailableMemories` section and that it directs the agent to `list_memories`.

## Related prompts
`src/prompts.rs` also holds two other agent-facing texts. Neither is returned by this tool:
- `connection_instructions(memory_only)`: the MCP server-level instructions, attached in `ServerHandler::get_info` (`src/server/mod.rs`) via `with_instructions`. Two variants (full and memory-only). This is where the "call `initial_instructions` first" requirement is stated to every MCP client.
- `session_brief(memory_only)`: the short text the `biskit-mcp hook session-start` CLI (`run_session_start_hook` in `src/main.rs`) prints as Claude Code SessionStart `additionalContext`. It is built from `SESSION_BRIEF`, which only demands calling `initial_instructions` first. Outside memory-only mode, `SYMBOLIC_TOOLS_BRIEF` is appended, directing the agent to Biskit's symbolic tools over Grep, Glob, and whole-file reads. The brief never contains the manual, so the hook and this tool do not deliver duplicate content.
- The hook entry is written into `.claude/settings.local.json` (or `.claude/settings.json` with `--hooks-target shared`) by `src/setup.rs`, invoked from `biskit-mcp setup --hooks` and the installers (`install.sh`, `install.ps1`).

## Edge cases
- Returned via `Biskit::text` (`src/server/mod.rs`), so it is truncated with a `[truncated: N of M characters shown, limited by tools.max_answer_chars]` footer when it exceeds `tools.max_answer_chars` (default set in `ToolSettings`'s `Default` in `src/config.rs`). A limit of `0` disables the ceiling.
- In memory-only mode, `Biskit::new` removes the routes in `LANGUAGE_SERVER_TOOLS` and `WALLY_TOOLS`. The memory-only manual names the language-server tools as absent; it does not mention the Wally tools.
- `initial_instructions` can itself be removed through `tools.excluded`, like any other route.

## Relevant files
- `src/server/mod.rs`
- `src/server/descriptions.rs`
- `src/prompts.rs`
- `src/main.rs`
- `src/setup.rs`
- `src/config.rs`
- `assets/instructions.md`
- `assets/instructions.memory-only.md`

Siblings: `mem:Tools/list_memories`, `mem:Tools/read_memory`, `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`.
