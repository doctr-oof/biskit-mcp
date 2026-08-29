use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::bail_hint;
use crate::config::WallySettings;
use crate::project::Project;

const NOT_ON_PATH_HINT: &str = "install Wally yourself from https://wally.run and make sure it is \
                                on PATH, or set wally.binary_path in .biskit/settings.yml. Biskit \
                                never installs it for you.";

const NOT_RUNNABLE_HINT: &str = "the file is on PATH but would not run. Version managers such as \
                                 Aftman and Rokit install a shim that refuses until the tool is \
                                 listed in their manifest, so check that first.";

/// The Wally executable this project will use, once it has proven it runs.
#[derive(Debug, Clone, Serialize)]
pub struct WallyProgram {
    pub path: String,
    pub version: String,
    #[serde(skip)]
    binary: PathBuf,
}

fn executable_name() -> &'static str {
    if cfg!(windows) { "wally.exe" } else { "wally" }
}

/// The configured binary, or the first `wally` on PATH.
fn discover(settings: &WallySettings) -> Result<PathBuf> {
    if let Some(configured) = &settings.binary_path {
        if configured.is_file() {
            return Ok(configured.clone());
        }
        bail_hint!(
            "point wally.binary_path at the Wally executable itself, or unset it to search PATH";
            "wally.binary_path does not point at a file: {}",
            configured.display()
        );
    }

    let Some(path) = std::env::var_os("PATH") else {
        bail_hint!(NOT_ON_PATH_HINT; "Wally is not available: this process has no PATH to search");
    };

    std::env::split_paths(&path)
        .map(|directory| directory.join(executable_name()))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            crate::errors::hinted(
                "Wally is not available: no `wally` executable is on PATH".to_string(),
                NOT_ON_PATH_HINT,
            )
        })
}

/// Finds Wally and confirms it runs, because a version-manager shim is on PATH without working.
pub fn locate(settings: &WallySettings) -> Result<WallyProgram> {
    let binary = discover(settings)?;

    let output = std::process::Command::new(&binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            crate::errors::hinted(
                format!(
                    "Wally is not available: could not run {}: {error}",
                    binary.display()
                ),
                NOT_RUNNABLE_HINT,
            )
        })?;

    if !output.status.success() {
        let reason = first_line(&merge(&output.stdout, &output.stderr))
            .unwrap_or_else(|| format!("exited with {}", output.status));
        bail_hint!(
            NOT_RUNNABLE_HINT;
            "Wally is not available: {} --version failed: {reason}",
            binary.display()
        );
    }

    Ok(WallyProgram {
        path: binary.display().to_string(),
        version: first_line(&String::from_utf8_lossy(&output.stdout))
            .unwrap_or_else(|| "unknown".to_string()),
        binary,
    })
}

/// Runs `wally install` in the project root and returns what it printed.
///
/// Wally deletes and rebuilds Packages, ServerPackages, and DevPackages on every install, so this
/// is never an additive operation however small the manifest change that prompted it.
pub async fn install(
    program: &WallyProgram,
    project: &Project,
    settings: &WallySettings,
) -> Result<String> {
    let mut command = tokio::process::Command::new(&program.binary);
    command
        .arg("install")
        .current_dir(project.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let failed = |error| anyhow!("failed to run `wally install`: {error}");
    let output = match settings.install_timeout_ms {
        0 => command.output().await.map_err(failed)?,
        milliseconds => {
            let deadline = Duration::from_millis(milliseconds);
            match tokio::time::timeout(deadline, command.output()).await {
                Ok(output) => output.map_err(failed)?,
                Err(_) => bail_hint!(
                    "raise wally.install_timeout_ms in .biskit/settings.yml, or run `wally \
                     install` yourself";
                    "`wally install` did not finish within {milliseconds}ms"
                ),
            }
        }
    };

    let printed = strip_ansi(&merge(&output.stdout, &output.stderr));
    if !output.status.success() {
        bail_hint!(
            "fix what the output above names, then run `wally install`";
            "`wally install` failed: {}\n\n{}",
            output.status,
            printed.trim()
        );
    }
    Ok(printed.trim().to_string())
}

fn merge(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, _) => stderr.into_owned(),
        (_, true) => stdout.into_owned(),
        _ => format!("{}\n{}", stdout.trim_end(), stderr.trim_end()),
    }
}

fn first_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Wally paints its progress output, and the escape sequences are noise in a tool result.
fn strip_ansi(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();

    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            cleaned.push(character);
            continue;
        }
        match characters.next() {
            Some('[') => {
                for escaped in characters.by_ref() {
                    if escaped.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(escaped) = characters.next() {
                    if escaped == '\u{7}' {
                        break;
                    }
                    if escaped == '\u{1b}' && characters.peek() == Some(&'\\') {
                        characters.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_are_dropped_and_text_is_kept() {
        let painted = "\u{1b}[32m Resolving \u{1b}[0mpackages...\n   Resolved 4 dependencies";
        assert_eq!(
            strip_ansi(painted),
            " Resolving packages...\n   Resolved 4 dependencies"
        );
    }

    #[test]
    fn an_unterminated_escape_does_not_leak_the_escape_character() {
        assert!(!strip_ansi("done\u{1b}[").contains('\u{1b}'));
    }

    #[test]
    fn a_binary_path_that_is_not_a_file_is_refused_before_anything_runs() {
        let settings = WallySettings {
            binary_path: Some(PathBuf::from("/definitely/not/here/wally")),
            ..Default::default()
        };
        let error = discover(&settings).unwrap_err();
        assert!(error.to_string().contains("does not point at a file"));
    }

    #[test]
    fn the_first_non_empty_line_is_what_reports_a_version() {
        assert_eq!(
            first_line("\n\nwally 0.3.2\nmore"),
            Some("wally 0.3.2".into())
        );
        assert_eq!(first_line("   \n"), None);
    }
}
