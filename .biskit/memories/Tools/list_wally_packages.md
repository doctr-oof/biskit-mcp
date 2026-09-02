Reports what `wally.toml` declares, joined to what `wally.lock` resolved and what is on disk. Disabled in this session: it is one of the four `WALLY_TOOLS` dropped from the router while `project.memory_only` is true (`src/server/mod.rs:116`, `src/server/mod.rs:134`).

## Parameters
None. The handler takes `NoArguments {}` (`src/server/requests.rs:418`).

## Implementation
- Handler: `src/server/mod.rs:694`, calls `WallyTools::list` (`src/wally/mod.rs:117`).
- `WallyTools::list` first runs `cli::locate` (`src/wally/cli.rs:62`) even though the answer is read from files: Wally must be found and must answer `--version` before the tool returns anything. Result carries that `WallyProgram { path, version }`.
- The rest runs in `spawn_blocking`: `Manifest::load` parses `wally.toml` with `toml_edit` (`src/wally/manifest.rs:277`), `Lockfile::load` parses `wally.lock` (`src/wally/manifest.rs:673`), and `Manifest::declared` walks `[dependencies]`, `[server-dependencies]`, `[dev-dependencies]` in that fixed realm order (`src/wally/manifest.rs:347`).
- Per entry (`describe`, `src/wally/manifest.rs:371`): the requirement string is split into name and version, the lockfile supplies `resolved_version`, and `install_path` is built as `<realm dir>/_Index/<scope>_<name>@<version>` where realm dir is `Packages`, `ServerPackages`, or `DevPackages` (`src/wally/manifest.rs:47`).
- `installed` is not "the index directory exists": it is `<index dir>/<package name>` existing, because a failed install leaves an empty index directory behind. `install_path` is reported only when the index directory itself exists.
- `lock_out_of_date` is set when the locked `Version` no longer satisfies the `VersionReq` in `wally.toml`.
- No registry request is made; no network at all.

## Edge cases
- No `wally.toml` in the project root: error with the hint to run `wally init` (`src/wally/manifest.rs:280`).
- Wally not on PATH, `wally.binary_path` not a file, or a version-manager shim that refuses to run: the call fails before any file is read (`src/wally/cli.rs:34`, `src/wally/cli.rs:62`).
- Listing-level `note` is chosen by (lockfile present, declared count, missing count): "declares no dependencies yet" wins first, then "there is no wally.lock", then "N declared package(s) are not on disk"; absent when everything is installed.
- A dependency value that is not a string produces a placeholder entry with a note rather than failing the whole listing (`unreadable`, `src/wally/manifest.rs:744`); a string that is not `SCOPE/NAME@VERSION` gets a similar per-entry note.
- Lockfile resolution prefers the alias recorded under the manifest's own `[package] name` root entry; failing that it falls back to a package-name lookup, and only when exactly one version is recorded for that name (`src/wally/manifest.rs:733`). A missing or renamed `[package] name` therefore silently degrades resolution.
- Result is serialised JSON and is subject to `tools.max_answer_chars`: an oversized listing is refused, not truncated (`src/server/mod.rs:60`).

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/wally/mod.rs`
- `src/wally/manifest.rs`
- `src/wally/cli.rs`
- `src/config.rs`
- `src/project.rs`

Siblings: `mem:Tools/search_wally_packages`, `mem:Tools/add_wally_package`, `mem:Tools/remove_wally_package`, `mem:Tools/get_status`.
