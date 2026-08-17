use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use ureq::ResponseExt;

use crate::config::LspSettings;

const BINARY_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const SUPPORT_FILE_HOSTS: [&str; 1] = ["luau-lsp.pages.dev"];
const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LanguageServerInstall {
    pub binary: PathBuf,
    pub definition_files: Vec<(String, PathBuf)>,
    pub documentation_files: Vec<PathBuf>,
}

pub fn asset_name() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "luau-lsp-win64.zip",
        ("macos", _) => "luau-lsp-macos.zip",
        ("linux", "x86_64") => "luau-lsp-linux-x86_64.zip",
        ("linux", "aarch64" | "arm") => "luau-lsp-linux-arm64.zip",
        (os, arch) => bail!("no luau-lsp release asset is published for {os}/{arch}"),
    })
}

fn binary_file_name() -> &'static str {
    if cfg!(windows) {
        "luau-lsp.exe"
    } else {
        "luau-lsp"
    }
}

pub fn install_root(settings: &LspSettings) -> Result<PathBuf> {
    let directories = directories::ProjectDirs::from("", "", "biskit")
        .ok_or_else(|| anyhow!("could not determine a cache directory for this platform"))?;
    Ok(directories
        .cache_dir()
        .join("language-server")
        .join(&settings.version))
}

pub fn ensure_installed(settings: &LspSettings) -> Result<LanguageServerInstall> {
    let root = install_root(settings)?;
    std::fs::create_dir_all(&root)?;

    let binary = match &settings.binary_path {
        Some(path) => {
            if !path.is_file() {
                bail!(
                    "lsp.binary_path does not point at a file: {}",
                    path.display()
                );
            }
            path.clone()
        }
        None => ensure_binary(settings, &root)?,
    };

    let mut definition_files = Vec::new();
    let mut documentation_files = Vec::new();

    if settings.wants_roblox_definitions() {
        let name = format!(
            "globalTypes.{}.d.luau",
            settings.roblox_security_level.as_str()
        );
        if let Some(path) = ensure_support_file(&settings.type_definitions_url(), &root, &name) {
            definition_files.push(("@roblox".to_string(), path));
        }
    }

    let documentation_url = settings.documentation_url();
    let documentation_name = documentation_url
        .rsplit('/')
        .next()
        .unwrap_or("api-docs.json")
        .to_string();
    if let Some(path) = ensure_support_file(documentation_url, &root, &documentation_name) {
        documentation_files.push(path);
    }

    Ok(LanguageServerInstall {
        binary,
        definition_files,
        documentation_files,
    })
}

fn ensure_binary(settings: &LspSettings, root: &Path) -> Result<PathBuf> {
    let binary = root.join(binary_file_name());
    if binary.is_file() {
        return Ok(binary);
    }

    let asset = asset_name()?;
    let url = match &settings.download_url_template {
        Some(template) => template
            .replace("{version}", &settings.version)
            .replace("{asset}", asset),
        None => format!(
            "https://github.com/{}/releases/download/{}/{asset}",
            settings.repository, settings.version
        ),
    };

    let expected = settings.checksum_for(asset);
    if expected.is_none() && settings.require_checksum {
        bail!(
            "no SHA-256 digest is known for {asset} at version {}. Add one under lsp.checksums, \
             or set lsp.require_checksum to false to accept an unverified download.",
            settings.version
        );
    }

    tracing::info!(target: "biskit::lsp", "downloading {url}");
    let archive = download(&url, &BINARY_HOSTS)?;

    if let Some(expected) = expected {
        let actual = hex_digest(&archive);
        if !actual.eq_ignore_ascii_case(&expected) {
            bail!(
                "checksum mismatch for {asset}: expected {expected}, got {actual}. \
                 The download was discarded."
            );
        }
    }

    extract_binary(&archive, root, &binary)?;
    Ok(binary)
}

/// Support files are unversioned and unhashed upstream, so a failure here is not fatal.
fn ensure_support_file(url: &str, root: &Path, file_name: &str) -> Option<PathBuf> {
    let destination = root.join(file_name);
    if destination.is_file() {
        return Some(destination);
    }
    match download(url, &SUPPORT_FILE_HOSTS).and_then(|body| {
        std::fs::write(&destination, body)?;
        Ok(())
    }) {
        Ok(()) => Some(destination),
        Err(error) => {
            tracing::warn!(target: "biskit::lsp", "could not fetch {url}: {error}");
            None
        }
    }
}

pub fn download(url: &str, allowed_hosts: &[&str]) -> Result<Vec<u8>> {
    check_host(url, allowed_hosts)?;

    let response = ureq::get(url)
        .header(
            "User-Agent",
            concat!("biskit-mcp/", env!("CARGO_PKG_VERSION")),
        )
        .config()
        .save_redirect_history(true)
        .build()
        .call()
        .with_context(|| format!("request failed: {url}"))?;

    // Redirects are followed internally, so every hop has to clear the allowlist too.
    match response.get_redirect_history() {
        Some(history) => {
            for hop in history {
                check_host(&hop.to_string(), allowed_hosts)?;
            }
        }
        None => check_host(&response.get_uri().to_string(), allowed_hosts)?,
    }

    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES)
        .read_to_end(&mut body)?;

    if body.is_empty() {
        bail!("downloaded an empty response from {url}");
    }
    Ok(body)
}

fn check_host(url: &str, allowed_hosts: &[&str]) -> Result<()> {
    let without_scheme = url
        .strip_prefix("https://")
        .ok_or_else(|| anyhow!("refusing a non-HTTPS URL: {url}"))?;
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

    if allowed_hosts.contains(&host) {
        return Ok(());
    }
    bail!("refusing to download from an unexpected host: {host}")
}

pub fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn extract_binary(archive: &[u8], root: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .context("release asset is not a valid zip archive")?;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            bail!("release asset contains an unsafe entry path");
        };
        if enclosed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("release asset contains a path traversal entry");
        }

        let name = enclosed
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name != binary_file_name() {
            continue;
        }

        let staged = root.join(format!("{}.partial", binary_file_name()));
        let mut writer = std::fs::File::create(&staged)?;
        std::io::copy(&mut entry, &mut writer)?;
        writer.sync_all()?;
        drop(writer);

        make_executable(&staged)?;
        std::fs::rename(&staged, destination)?;
        return Ok(());
    }

    bail!(
        "release asset did not contain a {} executable",
        binary_file_name()
    )
}

#[cfg(unix)]
pub fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allowlist_rejects_lookalikes() {
        assert!(check_host("https://github.com/a/b", &BINARY_HOSTS).is_ok());
        assert!(check_host("https://github.com.evil.tld/a", &BINARY_HOSTS).is_err());
        assert!(check_host("https://evil.tld/@github.com/a", &BINARY_HOSTS).is_err());
        assert!(check_host("http://github.com/a", &BINARY_HOSTS).is_err());
    }

    #[test]
    fn userinfo_cannot_spoof_the_host() {
        assert!(check_host("https://github.com@evil.tld/a", &BINARY_HOSTS).is_err());
    }

    #[test]
    fn digest_matches_known_vector() {
        assert_eq!(
            hex_digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
