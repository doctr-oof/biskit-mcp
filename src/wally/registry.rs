use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use ureq::ResponseExt;

use crate::bail_hint;
use crate::config::WallySettings;
use crate::lsp::acquire;
use crate::wally::manifest::PackageName;

const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// A registry answer that was not a success, kept apart from transport failures so a 404 reads as
/// "no such package" rather than as a network fault.
#[derive(Debug)]
struct RegistryStatus {
    status: u16,
    url: String,
}

impl std::fmt::Display for RegistryStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the registry answered HTTP {} for {}",
            self.status, self.url
        )
    }
}

impl std::error::Error for RegistryStatus {}

/// One package the registry's search index matched.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub package: String,
    pub scope: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The newest published version, which is what `add_wally_package` would pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub version_count: usize,
}

#[derive(Debug, Deserialize)]
struct RawSearchHit {
    scope: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawMetadata {
    #[serde(default)]
    versions: Vec<RawMetadataVersion>,
}

#[derive(Debug, Deserialize)]
struct RawMetadataVersion {
    package: RawMetadataPackage,
}

#[derive(Debug, Deserialize)]
struct RawMetadataPackage {
    version: String,
}

/// Talks to the Wally registry, one request at a time and no faster than the configured pace.
///
/// The registry publishes no rate limits and runs no limiter of its own, so the ceiling here is
/// Biskit's own restraint rather than something the server would enforce.
pub struct RegistryClient {
    settings: WallySettings,
    throttle: tokio::sync::Mutex<Throttle>,
    cache: Mutex<Cache>,
}

/// Registry answers keyed by URL, each stamped with when it was stored.
type Cache = HashMap<String, (Instant, Arc<Vec<u8>>)>;

struct Throttle {
    previous: Option<Instant>,
    window: VecDeque<Instant>,
}

impl RegistryClient {
    pub fn new(settings: WallySettings) -> Self {
        Self {
            settings,
            throttle: tokio::sync::Mutex::new(Throttle {
                previous: None,
                window: VecDeque::new(),
            }),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let url = format!(
            "{}v1/package-search?query={}",
            self.base(),
            percent_encode(query)
        );
        let body = self.fetch(url).await?;

        let raw: Vec<RawSearchHit> = serde_json::from_slice(&body)
            .context("the registry returned a search result Biskit could not read")?;

        Ok(raw
            .into_iter()
            .map(|hit| SearchHit {
                package: format!("{}/{}", hit.scope, hit.name),
                latest_version: newest(hit.versions.iter().map(String::as_str)),
                version_count: hit.versions.len(),
                scope: hit.scope,
                name: hit.name,
                description: hit.description.filter(|text| !text.trim().is_empty()),
            })
            .collect())
    }

    /// The newest published non-prerelease version of a package.
    pub async fn latest_version(&self, name: &PackageName) -> Result<String> {
        let url = format!(
            "{}v1/package-metadata/{}/{}",
            self.base(),
            name.scope(),
            name.name()
        );
        let body = self.fetch(url).await.map_err(|error| {
            match error.downcast_ref::<RegistryStatus>().map(|it| it.status) {
                Some(404) => crate::errors::hinted(
                    format!("{name} is not published in the registry"),
                    "check the scope and the name with search_wally_packages; both are lower case, \
                     and a package is named SCOPE/NAME",
                ),
                _ => crate::errors::hinted(
                    format!("could not look up {name} in the registry: {error}"),
                    "check the name with search_wally_packages, or pass version to skip the lookup",
                ),
            }
        })?;

        let metadata: RawMetadata = serde_json::from_slice(&body)
            .context("the registry returned metadata Biskit could not read")?;

        let versions: Vec<&str> = metadata
            .versions
            .iter()
            .map(|entry| entry.package.version.as_str())
            .collect();

        newest(versions.iter().copied()).ok_or_else(|| {
            crate::errors::hinted(
                match versions.is_empty() {
                    true => format!("{name} has no published versions"),
                    false => format!(
                        "{name} has only prerelease versions ({}), which are never chosen \
                         automatically",
                        versions.join(", ")
                    ),
                },
                "pass version explicitly to take one of them",
            )
        })
    }

    fn base(&self) -> String {
        let base = self.settings.registry_api_url.trim_end_matches('/');
        format!("{base}/")
    }

    async fn fetch(&self, url: String) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.cached(&url) {
            return Ok(hit);
        }

        let host = host_of(&url)?;
        let timeout = self.settings.request_timeout_ms;

        // The lock is held across the wait and the request, so requests to the registry are
        // serialised rather than merely paced: two tool calls cannot each think they are first.
        let body = {
            let mut throttle = self.throttle.lock().await;
            let wait = throttle.wait_for(&self.settings);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            throttle.record();

            let requested = url.clone();
            tokio::task::spawn_blocking(move || get(&requested, &host, timeout))
                .await
                .map_err(|error| anyhow!("the registry request panicked: {error}"))??
        };

        let body = Arc::new(body);
        self.store(url, Arc::clone(&body));
        Ok(body)
    }

    fn cached(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        let ttl = Duration::from_secs(self.settings.cache_ttl_seconds);
        if ttl.is_zero() {
            return None;
        }
        let cache = self.cache.lock().ok()?;
        let (stored, body) = cache.get(url)?;
        (stored.elapsed() < ttl).then(|| Arc::clone(body))
    }

    fn store(&self, url: String, body: Arc<Vec<u8>>) {
        let ttl = Duration::from_secs(self.settings.cache_ttl_seconds);
        if ttl.is_zero() {
            return;
        }
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        cache.retain(|_, (stored, _)| stored.elapsed() < ttl);
        cache.insert(url, (Instant::now(), body));
    }
}

impl Throttle {
    /// How long to wait before the next request may go out.
    fn wait_for(&mut self, settings: &WallySettings) -> Duration {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        while self
            .window
            .front()
            .is_some_and(|stamp| now.duration_since(*stamp) >= window)
        {
            self.window.pop_front();
        }

        let spacing = match self.previous {
            Some(previous) => Duration::from_millis(settings.min_request_interval_ms)
                .checked_sub(now.duration_since(previous))
                .unwrap_or_default(),
            None => Duration::ZERO,
        };

        let ceiling = settings.max_requests_per_minute;
        let quota = match ceiling > 0 && self.window.len() >= ceiling {
            true => self
                .window
                .front()
                .map(|oldest| window.saturating_sub(now.duration_since(*oldest)))
                .unwrap_or_default(),
            false => Duration::ZERO,
        };

        spacing.max(quota)
    }

    fn record(&mut self) {
        let now = Instant::now();
        self.previous = Some(now);
        self.window.push_back(now);
    }
}

/// A plain HTTPS GET with the same host and redirect checks the language server download uses.
fn get(url: &str, host: &str, timeout_ms: u64) -> Result<Vec<u8>> {
    acquire::check_host(url, &[host])?;

    let mut builder = ureq::get(url)
        .header(
            "User-Agent",
            concat!("biskit-mcp/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/json")
        .config()
        .save_redirect_history(true);
    if timeout_ms > 0 {
        builder = builder.timeout_global(Some(Duration::from_millis(timeout_ms)));
    }

    let response = match builder.build().call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Err(anyhow::Error::new(RegistryStatus {
                status,
                url: url.to_string(),
            }));
        }
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!("request failed: {url}")));
        }
    };

    match response.get_redirect_history() {
        Some(history) => {
            for hop in history {
                acquire::check_host(&hop.to_string(), &[host])?;
            }
        }
        None => acquire::check_host(&response.get_uri().to_string(), &[host])?,
    }

    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut body)?;
    Ok(body)
}

fn host_of(url: &str) -> Result<String> {
    let Some(without_scheme) = url.strip_prefix("https://") else {
        bail_hint!(
            "wally.registry_api_url must be an https:// URL";
            "refusing a non-HTTPS registry URL: {url}"
        );
    };
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();

    if host.is_empty() {
        bail_hint!(
            "wally.registry_api_url must name a host, such as \"https://api.wally.run/\"";
            "the registry URL names no host: {url}"
        );
    }
    Ok(host.to_string())
}

/// The highest released version, ignoring prereleases so `add` never pins an alpha by accident.
fn newest<'a>(versions: impl Iterator<Item = &'a str>) -> Option<String> {
    versions
        .filter_map(|version| semver::Version::parse(version).ok())
        .filter(|version| version.pre.is_empty())
        .max()
        .map(|version| version.to_string())
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prereleases_never_win_the_latest_slot() {
        let versions = ["1.4.2", "2.0.0-alpha.1", "1.10.0", "1.9.0"];
        assert_eq!(newest(versions.into_iter()), Some("1.10.0".to_string()));
    }

    #[test]
    fn versions_are_ordered_by_semver_not_by_text() {
        assert_eq!(
            newest(["0.9.0", "0.10.0"].into_iter()),
            Some("0.10.0".to_string()),
            "0.10.0 sorts below 0.9.0 as text and above it as a version"
        );
    }

    #[test]
    fn a_package_with_only_prereleases_has_no_latest() {
        assert_eq!(newest(["1.0.0-rc.1"].into_iter()), None);
    }

    #[test]
    fn queries_that_need_escaping_are_escaped() {
        assert_eq!(percent_encode("roblox/roact"), "roblox%2Froact");
        assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(percent_encode("plain-name_1.0~"), "plain-name_1.0~");
    }

    #[test]
    fn the_host_is_taken_from_the_configured_url() {
        assert_eq!(host_of("https://api.wally.run/").unwrap(), "api.wally.run");
        assert_eq!(
            host_of("https://api.wally.run:443/v1/x?q=1").unwrap(),
            "api.wally.run"
        );
        assert!(host_of("http://api.wally.run/").is_err());
    }

    #[test]
    fn a_userinfo_prefix_cannot_disguise_the_host() {
        assert_eq!(
            host_of("https://api.wally.run@evil.tld/v1").unwrap(),
            "evil.tld",
            "the host is what follows the last @, so the allowlist is built from evil.tld"
        );
    }

    fn settings(interval_ms: u64, per_minute: usize) -> WallySettings {
        WallySettings {
            min_request_interval_ms: interval_ms,
            max_requests_per_minute: per_minute,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_request_is_never_delayed() {
        let mut throttle = Throttle {
            previous: None,
            window: VecDeque::new(),
        };
        assert!(throttle.wait_for(&settings(500, 60)).is_zero());
    }

    #[test]
    fn a_second_request_waits_out_the_minimum_interval() {
        let mut throttle = Throttle {
            previous: None,
            window: VecDeque::new(),
        };
        throttle.record();
        let wait = throttle.wait_for(&settings(500, 60));
        assert!(
            wait > Duration::from_millis(400) && wait <= Duration::from_millis(500),
            "unexpected wait: {wait:?}"
        );
    }

    #[test]
    fn the_per_minute_ceiling_holds_once_the_window_is_full() {
        let mut throttle = Throttle {
            previous: None,
            window: VecDeque::new(),
        };
        for _ in 0..3 {
            throttle.record();
        }

        let wait = throttle.wait_for(&settings(0, 3));
        assert!(
            wait > Duration::from_secs(59),
            "a full window must wait for it to roll, got {wait:?}"
        );
    }

    #[test]
    fn a_zero_ceiling_lifts_the_per_minute_limit() {
        let mut throttle = Throttle {
            previous: None,
            window: VecDeque::new(),
        };
        for _ in 0..500 {
            throttle.record();
        }
        assert!(throttle.wait_for(&settings(0, 0)).is_zero());
    }
}
