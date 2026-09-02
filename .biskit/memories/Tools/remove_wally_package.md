Drops a dependency from `wally.toml` and, by default, rebuilds the package tree with `wally install`. Disabled in this session: one of the four `WALLY_TOOLS` dropped while `project.memory_only` is true (`src/server/mod.rs:116`).

## Parameters
`RemoveWallyPackageRequest` (`src/server/requests.rs:403`):
- `package`: string, required. Either `scope/name` or the alias it was declared under — both are matched (`declarations_of`, `src/wally/manifest.rs:597`).
- `realm`: string, optional. `shared`/`server`/`dev` (or the full section names). Omitted searches all three realms; only needed for a hand-written manifest that declares the same package twice.
- `install`: bool, default `true`.

## Implementation
- Handler: `src/server/mod.rs:753`, builds `RemoveRequest` and calls `WallyTools::remove` (`src/wally/mod.rs:272`).
- `realm` is parsed to `Option<Realm>` before the blocking work; the manifest edit (`Manifest::remove`, `src/wally/manifest.rs:540`) runs in `spawn_blocking` and `save`s the `toml_edit` document, preserving formatting.
- Matching is exact string equality against either the table key (alias) or the parsed package name from the requirement value.
- Optional install goes through the same `install_or_roll_back` path as `mem:Tools/add_wally_package`: on failure the original `wally.toml` is restored, missing-from-disk packages are named, and the tree warning is appended.
- The returned `ManifestChange` reuses the add shape: `alias`, `package`, `version_req` (as it was declared), `realm`, `section`, `replaced` always `None`, `install_ran`, `install_output`.

## Edge cases
- Nothing matches, and a `realm` was given, but the package exists in another realm: the error names where it actually sits rather than claiming it is undeclared (`src/wally/manifest.rs:551`).
- Nothing matches anywhere: error hinting at `list_wally_packages`.
- More than one match (same name/alias across realms): refused with "pass realm to say which one to remove".
- `install: false`: the note says the manifest was edited without an install *and* that the removed package is still on disk.
- With `install: true` the whole package tree is deleted and rebuilt, so unrelated packages are momentarily gone and stay gone if the install fails.
- No `wally.toml`: same `Manifest::load` error as the other Wally tools. Wally must still be locatable and runnable (`cli::locate`) before any file is touched.
- A version requirement that will not split leaves `version_req` empty rather than failing.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/wally/mod.rs`
- `src/wally/manifest.rs`
- `src/wally/cli.rs`
- `src/config.rs`
- `src/project.rs`

Siblings: `mem:Tools/add_wally_package`, `mem:Tools/list_wally_packages`, `mem:Tools/search_wally_packages`.
