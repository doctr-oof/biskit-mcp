Reports Biskit's own state: which project it serves, how that root was chosen, mode, language-server state, sourcemap freshness, memory count, and every setting that differs from the defaults. Registered in both modes — a router test asserts it survives `project.memory_only` (`src/server/mod.rs:937`).

## Parameters
None. `NoArguments {}` (`src/server/requests.rs:418`).

## Implementation
- Handler: `src/server/mod.rs:774`, calls `status::collect` (`src/status.rs:114`) with the language-server handle, the Roblox index, settings, the memory store, and `root_source`.
- `Status` fields (`src/status.rs:18`):
  - `biskit_version` from `CARGO_PKG_VERSION`; `project_root` from the handle's `Project`.
  - `root_source`: how the root was chosen, decided at startup in `resolve_root` (`src/main.rs:187`) — one of `--project`, the project env var name, `working directory`, or `search upwards from the working directory`.
  - `mode`: `"memory-only"` or `"full"`.
  - `settings`: paths and presence of `.biskit/settings.yml` and the local override file, plus `overrides` — a `BTreeMap` of dotted keys built by diffing the serialised `Settings` against `Settings::default()` recursively (`collect_overrides`, `src/status.rs:181`). Only leaves that differ appear; the map is omitted when empty.
  - `language_server`: `state` label from `ServerState` (`disabled`, `running`, `not started`, `starting`, `exited, restarts on the next request`), pinned `version`, `repository`, `platform`, `roblox_security_level`, `binary` (present only if already downloaded, via `acquire::installed_binary`), and `request_timeout_ms`.
  - `sourcemap` and `shared_require`: both `None` in memory-only mode.
  - `memories`: count from `MemoryStore::list()` and the memories directory relative to the project root.
- `state()` never awaits the session: it `try_lock`s, so a lock held by a starting session is reported as `starting` rather than blocking (`src/lsp/session/handle.rs:181`).
- Sourcemap freshness (`sourcemap_status`, `src/status.rs:237`): reads `lsp.sourcemap`'s mtime; `stale` is true when `roblox.newest_source()` finds a Luau file written after that mtime, and the offending file is reported as `newest_source` so the claim can be checked. Missing file yields a note with the exact `rojo sourcemap` command.
- `shared_require_status` (`src/status.rs:208`) compares `project.shared_require` against the pinned LSP release: `supported` when the parsed version is at least `FIRST_SHARED_REQUIRE_VERSION`, `unsupported` when below, `unknown` when the version will not parse as three integers or `lsp.repository` is not the default. Notes are emitted only for the two disagreement cases (graph on + LSP unsupported, graph off + LSP supported).

## Edge cases
- This is the tool to call when another tool returns nothing and "no matches" cannot be told apart from "nothing is running" — that is its stated purpose in `src/server/descriptions.rs`.
- Absence is meaningful: no `sourcemap`/`shared_require` keys means memory-only mode, not a missing sourcemap; `stale` absent means the sourcemap file is absent.
- `overrides` are computed from serialised values, so any setting excluded from serialisation is invisible here.
- Nothing is verified by probing: `binary` is a file-existence check, not a working binary.
- Subject to `tools.max_answer_chars` like every other JSON result.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/status.rs`
- `src/lsp/session/handle.rs`
- `src/lsp/acquire.rs`
- `src/config.rs`
- `src/project.rs`
- `src/memory.rs`
- `src/roblox/sourcemap.rs`
- `src/main.rs`

Siblings: `mem:Tools/restart_language_server`, `mem:Tools/list_wally_packages`.
