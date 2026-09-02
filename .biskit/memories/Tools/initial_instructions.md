# initial_instructions

Returns Biskit's usage manual with this project's memory index appended.

## Parameters
None. The request type is `NoArguments {}` (`src/server/requests.rs:418`), an empty object.

## Implementation
- Handler: `src/server/mod.rs:185-200`. Calls `MemoryStore::list()`, then `prompts::initial_instructions(&memories, settings.project.memory_only)`, and returns the string through `Biskit::text`.
- `src/prompts.rs:42-77` builds the answer: picks the manual with `instructions_manual(memory_only)` — `assets/instructions.md` normally, `assets/instructions.memory-only.md` when `project.memory_only` is true (both `include_str!`ed at `src/prompts.rs:1-3`), trims trailing whitespace, strips the final `</Main>`, appends an `<Section name="AvailableMemories">` block, then re-closes `</Main>`.
- Memory index is names only, one `<Memory name="..." />` line per entry, in the sorted order `list()` returns (see `mem:Tools/list_memories`). With no memories it emits a single `<Rule>` telling the reader to consider writing one; otherwise it appends a rule saying to `read_memory` every plausibly relevant entry.
- Separate from this tool, the same module supplies `connection_instructions(memory_only)` (`src/prompts.rs:26-31`), the MCP server-level instructions attached in `ServerHandler::get_info` (`src/server/mod.rs:804-810`). That text is what the client sees before any tool call and is where the "call `initial_instructions` first" requirement is stated.

## Edge cases
- Returned via `text()` (`src/server/mod.rs:78-93`), so it is truncated with a `[truncated: N of M characters shown, limited by tools.max_answer_chars]` footer when it exceeds `tools.max_answer_chars` (default 50000, `src/config.rs:254`). A limit of `0` disables the ceiling.
- In memory-only mode the manual is the one that also documents the 15 language-server tools and the 4 Wally tools as absent; those routes are removed in `Biskit::new` (`src/server/mod.rs:96-145`).
- `list()` failure propagates as `initial_instructions failed: ...` via `fail("initial_instructions")`.
- The manual is compiled into the binary; editing `assets/instructions*.md` requires a rebuild.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/prompts.rs`
- `src/memory.rs`
- `src/config.rs`
- `assets/instructions.md`
- `assets/instructions.memory-only.md`

Siblings: `mem:Tools/list_memories`, `mem:Tools/read_memory`, `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`.
