use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::lsp::acquire::{download, hex_digest, make_executable};

const REPOSITORY: &str = "doctr-oof/biskit-mcp";
const SUMS_FILE: &str = "SHA256SUMS";
const RELEASE_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

pub fn run(tag: Option<String>) -> Result<()> {
    let current = std::env::current_exe().context("could not determine the running executable")?;
    let _ = std::fs::remove_file(sibling(&current, "old"));

    let asset = asset_name()?;
    let tag = resolve_tag(tag)?;
    let base = format!("https://github.com/{REPOSITORY}/releases/download/{tag}");

    println!("Downloading {asset} ({tag})...");
    let archive = download(&format!("{base}/{asset}"), &RELEASE_HOSTS)?;
    let sums = download(&format!("{base}/{SUMS_FILE}"), &RELEASE_HOSTS)?;
    let sums =
        String::from_utf8(sums).with_context(|| format!("{SUMS_FILE} is not valid UTF-8"))?;

    let Some(expected) = expected_digest(&sums, asset) else {
        bail!("{SUMS_FILE} does not list {asset}. Refusing to install.");
    };
    let actual = hex_digest(&archive);
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!(
            "checksum mismatch for {asset}: expected {expected}, got {actual}. \
             The download was discarded."
        );
    }
    println!("Checksum verified.");

    let binary = extract_binary(&archive, asset)?;
    install(&current, &binary)?;

    println!();
    println!(
        "biskit-mcp {} replaced with {tag}",
        env!("CARGO_PKG_VERSION")
    );
    println!("Installed to {}", current.display());
    println!("Project registrations were not touched.");
    Ok(())
}

fn binary_file_name() -> &'static str {
    if cfg!(windows) {
        "biskit-mcp.exe"
    } else {
        "biskit-mcp"
    }
}

fn asset_name() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "biskit-mcp-windows-x86_64.zip",
        ("macos", "aarch64") => "biskit-mcp-macos-aarch64.tar.gz",
        ("macos", "x86_64") => "biskit-mcp-macos-x86_64.tar.gz",
        ("linux", "x86_64") => "biskit-mcp-linux-x86_64.tar.gz",
        ("linux", "aarch64") => "biskit-mcp-linux-aarch64.tar.gz",
        (os, arch) => bail!("no biskit-mcp release asset is published for {os}/{arch}"),
    })
}

fn resolve_tag(requested: Option<String>) -> Result<String> {
    if let Some(value) = requested {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("--tag was empty");
        }
        return Ok(normalize_tag(trimmed));
    }

    let body = download(
        &format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
        &RELEASE_HOSTS,
    )?;
    let release: serde_json::Value =
        serde_json::from_slice(&body).context("could not parse the GitHub release response")?;
    let tag = release
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .context("the latest release does not name a tag")?;
    Ok(tag.to_string())
}

/// Releases are tagged `v0.1.4`, so a bare version number is accepted too.
fn normalize_tag(value: &str) -> String {
    if value.starts_with(|character: char| character.is_ascii_digit()) {
        return format!("v{value}");
    }
    value.to_string()
}

fn expected_digest(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        (name == asset).then(|| digest.to_ascii_lowercase())
    })
}

/// Entry paths are never written to disk, so only the file name matters here.
fn extract_binary(archive: &[u8], asset: &str) -> Result<Vec<u8>> {
    if asset.ends_with(".zip") {
        return extract_from_zip(archive);
    }
    extract_from_tar_gz(archive)
}

fn extract_from_zip(archive: &[u8]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .context("release asset is not a valid zip archive")?;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        if !names_the_binary(&enclosed) {
            continue;
        }

        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }

    bail!(
        "release asset did not contain a {} executable",
        binary_file_name()
    )
}

fn extract_from_tar_gz(archive: &[u8]) -> Result<Vec<u8>> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);

    for entry in tar
        .entries()
        .context("release asset is not a valid tar archive")?
    {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !names_the_binary(&path) {
            continue;
        }

        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }

    bail!(
        "release asset did not contain a {} executable",
        binary_file_name()
    )
}

fn names_the_binary(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == binary_file_name())
}

fn install(current: &Path, binary: &[u8]) -> Result<()> {
    let staged = sibling(current, "new");
    let backup = sibling(current, "old");

    std::fs::write(&staged, binary)
        .with_context(|| format!("could not write {}", staged.display()))?;
    make_executable(&staged)?;

    // Windows refuses to overwrite a running image but allows it to be renamed, so the
    // current binary is moved aside rather than replaced in place.
    if let Err(error) = std::fs::rename(current, &backup) {
        let _ = std::fs::remove_file(&staged);
        return Err(error).with_context(|| format!("could not move {} aside", current.display()));
    }

    if let Err(error) = std::fs::rename(&staged, current) {
        let _ = std::fs::rename(&backup, current);
        let _ = std::fs::remove_file(&staged);
        return Err(error)
            .with_context(|| format!("could not install the new {}", current.display()));
    }

    if std::fs::remove_file(&backup).is_err() {
        println!(
            "Left {} behind because it is still running; the next upgrade removes it.",
            backup.display()
        );
    }
    Ok(())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| binary_file_name().to_string());
    path.with_file_name(format!("{name}.{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_version_gains_the_tag_prefix() {
        assert_eq!(normalize_tag("0.1.4"), "v0.1.4");
        assert_eq!(normalize_tag("v0.1.4"), "v0.1.4");
    }

    #[test]
    fn the_digest_is_read_from_the_matching_line() {
        let sums = "aaaa  biskit-mcp-linux-x86_64.tar.gz\r\nBBBB  biskit-mcp-windows-x86_64.zip\n";

        assert_eq!(
            expected_digest(sums, "biskit-mcp-windows-x86_64.zip"),
            Some("bbbb".to_string())
        );
        assert_eq!(
            expected_digest(sums, "biskit-mcp-macos-aarch64.tar.gz"),
            None
        );
    }

    #[test]
    fn a_binary_marker_does_not_break_the_name_match() {
        let sums = "aaaa *biskit-mcp-linux-x86_64.tar.gz\n";

        assert_eq!(
            expected_digest(sums, "biskit-mcp-linux-x86_64.tar.gz"),
            Some("aaaa".to_string())
        );
    }

    #[test]
    fn a_name_that_only_ends_with_the_asset_is_not_matched() {
        let sums = "aaaa  nested/biskit-mcp-linux-x86_64.tar.gz\n";

        assert_eq!(
            expected_digest(sums, "biskit-mcp-linux-x86_64.tar.gz"),
            None
        );
    }

    #[test]
    fn the_staging_names_sit_beside_the_executable() {
        let current = Path::new("/opt/bin/biskit-mcp.exe");

        assert_eq!(
            sibling(current, "old"),
            Path::new("/opt/bin/biskit-mcp.exe.old")
        );
        assert_eq!(
            sibling(current, "new"),
            Path::new("/opt/bin/biskit-mcp.exe.new")
        );
    }
}
