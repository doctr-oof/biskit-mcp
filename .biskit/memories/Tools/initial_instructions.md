# initial_instructions

Returns Biskit's usage manual with this project's memory index appended.

## Parameters
`InitialInstructionsRequest` (`src/server/requests.rs:420-427`), one optional field:
- `force` (bool, default false) — send the full manual even when a SessionStart hook already delivered it. Without it the tool answers with a short stub in that case.

## Implementation
- Handler: `src/server/mod.rs:198-217`. Answers `prompts::ALREADY_DELIVERED` and stops when `!force && manual_already_delivered()`; otherwise calls `MemoryStore::list()`, then `prompts::initial_instructions(&memories, settings.project.memory_only)`, and returns the string through `Biskit::text`.
- `src/prompts.rs:76-111` builds the answer: picks the manual with `instructions_manual(memory_only)` — `assets/instructions.md` normally, `assets/instructions.memory-only.md` when `project.memory_only` is true (both `include_str!`ed at `src/prompts.rs:1-3`), trims trailing whitespace, strips the final `</Main>`, appends an `<Section name="AvailableMemories">` block, then re-closes `</Main>`.
- Memory index is names only, one `<Memory name="..." />` line per entry, in the sorted order `list()` returns (see `mem:Tools/list_memories`). With no memories it emits a single `<Rule>` telling the reader to consider writing one; otherwise it appends a rule saying to `read_memory` every plausibly relevant entry.
- Separate from this tool, the same module supplies `connection_instructions(memory_only, already_delivered)` (`src/prompts.rs:58-65`), the MCP server-level instructions attached in `ServerHandler::get_info` (`src/server/mod.rs:820-828`). That text is what the client sees before any tool call and is where the "call `initial_instructions` first" requirement is stated — four variants, because the delivered pair says "do NOT call `initial_instructions`" instead.

## Hook deduplication
The `biskit-mcp hook session-start` CLI (`src/main.rs:463-486`) emits the *same* string this tool returns, as Claude Code SessionStart `additionalContext`. Left alone, an agent obeying the connection instructions would then call this tool and receive a second copy — roughly 8.5k tokens of `assets/instructions.md` paid twice per session. So:

- After printing, the hook calls `session_start::record_delivery` (`src/session_start.rs`), writing `.biskit/cache/session-start.json` holding `delivered_at_ms`. Write failures warn on stderr and are otherwise swallowed; the tool then just answers in full.
- `Biskit::new` stamps `Inner.started_at_ms`. `manual_already_delivered()` (`src/server/mod.rs:192-196`) asks `session_start::delivered_for_session`, true only when `delivered_at_ms + TOLERANCE_MS >= started_at_ms` (tolerance 10 minutes).
- The anchor is deliberately the server's *start*, not the wall clock: the hook and the server launch together in an order neither controls, so the marker may land on either side of construction. A marker from an earlier session falls outside the window and is ignored.
- Detection is the runtime marker only, never the presence of a hook entry in `.claude/settings*.json`. Cursor and VS Code can carry that configuration without ever running the hook; they write no marker and correctly get the full manual.
- `get_info` races the hook, since MCP `initialize` can precede it. A miss costs one stub round-trip, not a duplicate manual — both orderings stay correct.
- The marker shares the LSP cache directory, created through `lsp::cache::prepare_dir`. Version control is handled one level up: `GITIGNORE_CONTENTS` in `src/project.rs:15` carries a `cache/` entry, and `Project::bootstrap` tops up an existing `.biskit/.gitignore` that predates it via `with_cache_entry` (`src/project.rs:237-253`), reported as `BootstrapReport::updated_gitignore`. Memory-only projects now create the cache directory for this marker and nothing else.

## Edge cases
- Returned via `text()` (`src/server/mod.rs:82-97`), so it is truncated with a `[truncated: N of M characters shown, limited by tools.max_answer_chars]` footer when it exceeds `tools.max_answer_chars` (default 50000, `src/config.rs:254`). A limit of `0` disables the ceiling.
- In memory-only mode the manual is the one that also documents the 15 language-server tools and the 4 Wally tools as absent; those routes are removed in `Biskit::new` (`src/server/mod.rs:129-180`).
- `list()` failure propagates as `initial_instructions failed: ...` via `fail("initial_instructions")`.
- The manual is compiled into the binary; editing `assets/instructions*.md` requires a rebuild.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/prompts.rs`
- `src/session_start.rs`
- `src/main.rs`
- `src/project.rs`
- `src/lsp/cache.rs`
- `src/memory.rs`
- `src/config.rs`
- `assets/instructions.md`
- `assets/instructions.memory-only.md`

Siblings: `mem:Tools/list_memories`, `mem:Tools/read_memory`, `mem:Tools/create_memory`, `mem:Tools/edit_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`.
