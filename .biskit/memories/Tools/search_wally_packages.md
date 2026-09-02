Searches the Wally registry by scope, name, and description, reporting each hit's newest published release. Disabled in this session: one of the four `WALLY_TOOLS` dropped while `project.memory_only` is true (`src/server/mod.rs:116`).

## Parameters
`SearchWallyPackagesRequest` (`src/server/requests.rs:362`):
- `query`: string, required. Matched by the registry against scope, name, and description.
- `max_results`: integer, optional. Defaults to `wally.max_search_results` (25) when omitted; `0` is refused rather than silently returning nothing.

## Implementation
- Handler: `src/server/mod.rs:711`; reads the default from `settings.wally.max_search_results` and calls `WallyTools::search` (`src/wally/mod.rs:157`).
- `search` calls `cli::locate` first, so a missing or unrunnable Wally binary fails the call even though the answer comes entirely from HTTP.
- `RegistryClient::search` (`src/wally/registry.rs:106`) GETs `{registry_api_url}v1/package-search?query=<percent-encoded>`; the encoder is hand-rolled and escapes everything outside `A-Za-z0-9-_.~` (`src/wally/registry.rs:356`).
- Truncation happens in Biskit, not the registry: the full match list is fetched, `matched_count` recorded, then cut to `max_results`. `truncated` and `matched_count` are only serialised when the cut happened.
- `latest_version` per hit comes from the hit's own `versions` array through `newest` (`src/wally/registry.rs:348`), which parses semver and **drops prereleases**, so a package with only prereleases reports no latest version.

## Edge cases
- Empty/whitespace `query` is refused with a hint before any request.
- `max_results: 0` is refused with a hint pointing at `wally.max_search_results`.
- Requests are serialised and paced by `Throttle` (`src/wally/registry.rs:233`) while holding a `tokio::sync::Mutex` across the sleep and the request: minimum gap `wally.min_request_interval_ms` (500ms) and a rolling-minute ceiling `wally.max_requests_per_minute` (60; `0` lifts it). A busy session therefore blocks rather than bursting.
- Answers are cached by URL for `wally.cache_ttl_seconds` (300s; `0` disables), so a repeated identical search may not hit the network (`src/wally/registry.rs:210`).
- Transport rules are shared with the language-server download: HTTPS only (`host_of` refuses `http://`), and every redirect hop is host-checked against the configured host via `acquire::check_host`. Response body is capped at 8 MiB (`MAX_RESPONSE_BYTES`).
- Note field: on truncation it names the counts; on zero matches it says the registry matches scope/name/description and suggests a shorter word; otherwise absent.
- Registry JSON that will not deserialise produces "the registry returned a search result Biskit could not read".

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/wally/mod.rs`
- `src/wally/registry.rs`
- `src/wally/cli.rs`
- `src/lsp/acquire.rs` (`check_host`)
- `src/config.rs` (`WallySettings`)

Siblings: `mem:Tools/list_wally_packages`, `mem:Tools/add_wally_package`, `mem:Tools/remove_wally_package`.
