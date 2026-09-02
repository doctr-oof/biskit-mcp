Tears down the Luau language server session and starts a fresh one. Disabled in this session: it is listed in `LANGUAGE_SERVER_TOOLS` (`src/server/mod.rs:96`) and dropped from the router while `project.memory_only` is true.

## Parameters
None. `NoArguments {}` (`src/server/requests.rs:418`).

## Implementation
- Handler: `src/server/mod.rs:789`, calls `LanguageServerHandle::restart` (`src/lsp/session/handle.rs:195`) and returns the plain text `"Language server restarted."` — not JSON, so it goes through `Biskit::text` and would be truncated rather than refused if it ever exceeded `tools.max_answer_chars`.
- `restart` is `stop().await` then `session().await`, so the replacement is started synchronously and any startup failure surfaces as the tool's error.
- `stop` (`src/lsp/session/handle.rs:200`) flushes the persistent symbol cache to disk first, then takes the session out of the mutex and calls `Session::shutdown`.
- `session()` (`src/lsp/session/handle.rs:92`) refuses immediately in memory-only mode, otherwise starts `Session::start` and, when `lsp.sync_disk_changes` is on, sweeps every `.luau`/`.lua` file under the root to seed disk stamps so the first sweep has a baseline. Startup time is logged.
- What survives a restart: the on-disk symbol cache (flushed, then reused), the project and settings held by the handle. What does not: the child process, its open documents, and everything it had inferred.

## Edge cases
- Not needed for a crashed server: `session()` already detects a dead session and starts a replacement on the next request, which is what the `exited, restarts on the next request` state label in `mem:Tools/get_status` means.
- A restart re-pays the full startup cost, including the disk-stamp baseline sweep over the whole project.
- Errors from acquiring or launching the binary propagate here (download, checksum, `lsp.binary_path` misconfiguration), so a failed restart can leave no session at all — the next request will try again.
- The state check in `get_status` uses `try_lock`, so a restart in flight reads as `starting`.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/lsp/session/handle.rs`
- `src/lsp/session/mod.rs`
- `src/lsp/acquire.rs`
- `src/lsp/cache.rs`
- `src/config.rs`
- `src/status.rs`

Sibling: `mem:Tools/get_status`.
