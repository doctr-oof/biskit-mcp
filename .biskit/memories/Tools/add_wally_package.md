Declares a package in `wally.toml` and, by default, runs `wally install`. Disabled in this session: one of the four `WALLY_TOOLS` dropped while `project.memory_only` is true (`src/server/mod.rs:116`).

## Parameters
`AddWallyPackageRequest` (`src/server/requests.rs:373`):
- `package`: string, required. `scope/name`, or `scope/name@^1.4.2` with the version inline.
- `version`: string, optional. Semver requirement. Omitted means one registry lookup for the newest published release.
- `realm`: string, optional, defaults to `shared`. Accepts `shared`/`dependencies`, `server`/`server-dependencies`, `dev`/`dev-dependencies`, case-insensitive (`src/wally/manifest.rs:55`).
- `alias`: string, optional. Defaults to the package name in PascalCase with hyphens stripped (`default_alias`, `src/wally/manifest.rs:196`) — `roblox/roact` becomes `Roact`.
- `overwrite`: bool, default `false`. Replaces an existing declaration; also what moves a package between realms.
- `install`: bool, default `true`.

## Implementation
- Handler: `src/server/mod.rs:729`, builds `AddRequest` and calls `WallyTools::add` (`src/wally/mod.rs:202`).
- Order of work: `cli::locate` → `split_requirement` → version resolution → `Realm::parse` → alias check → blocking manifest edit → optional install.
- Version resolution: inline and explicit versions that disagree are refused; either alone is validated with `check_version_req` (a real `semver::VersionReq` parse); neither means `RegistryClient::latest_version` and the requirement becomes `^<newest non-prerelease>` (`src/wally/mod.rs:206`).
- Manifest edit runs in `spawn_blocking` with `toml_edit`, keeping formatting. `Manifest::insert` (`src/wally/manifest.rs:466`) enforces the invariant that a package lives in exactly one realm: it sweeps every alias naming that package out of all three tables before writing the new entry, so a realm move or alias change leaves no duplicate. The displaced requirement is returned as `replaced`.
- Install: `cli::install` runs `wally install` in the project root with stdin null, capturing merged stdout/stderr with ANSI stripped, bounded by `wally.install_timeout_ms` (300000ms; `0` disables). `wally install` deletes and rebuilds `Packages`, `ServerPackages`, and `DevPackages` in full — it is never additive.
- Failure rollback (`install_or_roll_back`, `src/wally/mod.rs:319`): the pre-edit `wally.toml` text is written back, packages resolved in the lockfile but now absent from disk are enumerated (`missing_from_disk`), and the error is re-hinted with whether the rollback succeeded, the tree warning, the missing list, and — when the output mentions a shared dependency / shared packages placement — the `[place]` hint.

## Edge cases
- Package names are validated Wally-style: exactly one `/`, each segment 1–64 chars of lowercase ASCII, digits, or hyphen; an uppercase segment gets a "Wally names are lower case" hint (`check_segment`, `src/wally/manifest.rs:122`).
- Alias must read as a Luau identifier: non-empty, not starting with a digit, alphanumeric plus `_` (`check_alias`).
- Already declared without `overwrite`: two distinct errors — same realm ("pass overwrite"), different realm ("a package is declared in one realm only… pass overwrite to move it").
- A package with only prerelease versions, or none, fails `latest_version` with a hint to pass `version` explicitly; a 404 is reported as "not published in the registry" rather than a network fault.
- `install: false` adds `NO_INSTALL_NOTE` — manifest and package tree disagree until an install runs.
- Adding into `server` or `dev` when `wally.toml` has no `[place] shared-packages` adds `NO_PLACE_NOTE`; Wally refuses to link such a package against a shared one.
- `install_ran` reports only that `wally install` executed, not what is on disk — `mem:Tools/list_wally_packages` is the check for that.
- A `[dependencies]` key that exists but is not a table aborts the insert with a context error.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/wally/mod.rs`
- `src/wally/manifest.rs`
- `src/wally/registry.rs`
- `src/wally/cli.rs`
- `src/errors.rs` (`hinted`, `rehinted`, `bail_hint!`)
- `src/config.rs`
- `src/project.rs`

Siblings: `mem:Tools/remove_wally_package`, `mem:Tools/search_wally_packages`, `mem:Tools/list_wally_packages`.
