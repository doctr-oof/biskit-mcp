use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::config::Settings;
use crate::lsp::acquire;
use crate::lsp::session::LanguageServerHandle;
use crate::memory::MemoryStore;
use crate::project::{self, Project};

/// Everything a confused caller needs to tell "no matches" from "nothing is running".
///
/// `find_symbol` answering with nothing has three very different causes, and none of them is
/// visible from the empty result: the project may genuinely lack the symbol, the sourcemap may be
/// missing or stale, or the language server may have died. Each is reported here.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub biskit_version: &'static str,
    pub project_root: String,
    /// How the root was chosen, which is the first thing to doubt when the answers look wrong.
    pub root_source: String,
    pub mode: &'static str,
    pub settings: SettingsStatus,
    pub language_server: LanguageServerStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sourcemap: Option<SourcemapStatus>,
    pub memories: MemoryStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsStatus {
    pub settings_file: String,
    pub settings_file_present: bool,
    pub local_settings_file: String,
    pub local_settings_file_present: bool,
    /// Every setting that differs from Biskit's default, as dotted keys.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LanguageServerStatus {
    /// One of `running`, `not started`, `starting`, or `disabled`.
    pub state: &'static str,
    pub version: String,
    pub repository: String,
    pub platform: &'static str,
    pub roblox_security_level: &'static str,
    /// Absent when nothing has been downloaded yet. Reporting never triggers acquisition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourcemapStatus {
    pub relative_path: String,
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_epoch_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
    pub watched: bool,
    /// True when a Luau file has been written since the sourcemap was generated, which makes every
    /// DataModel-typed answer suspect until it is regenerated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// The file that makes it stale, so the claim can be checked rather than taken on faith.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStatus {
    pub count: usize,
    pub directory: String,
}

const MISSING_SOURCEMAP_NOTE: &str = "no sourcemap on disk, so instance paths and DataModel types \
                                      will not resolve. Generate one with `rojo sourcemap \
                                      --include-non-scripts --watch default.project.json --output \
                                      sourcemap.json`.";

const STALE_SOURCEMAP_NOTE: &str = "a Luau file is newer than the sourcemap, so a script added or \
                                    moved since then is invisible to the language server. \
                                    Regenerate the sourcemap.";

pub async fn collect(
    handle: &LanguageServerHandle,
    settings: &Settings,
    memories: &MemoryStore,
    root_source: &str,
) -> Result<Status> {
    let project = handle.project();
    let memory_only = settings.project.memory_only;

    Ok(Status {
        biskit_version: env!("CARGO_PKG_VERSION"),
        project_root: project.root().display().to_string(),
        root_source: root_source.to_string(),
        mode: if memory_only { "memory-only" } else { "full" },
        settings: settings_status(project, settings),
        language_server: LanguageServerStatus {
            state: handle.state().label(),
            version: settings.lsp.version.clone(),
            repository: settings.lsp.repository.clone(),
            platform: settings.lsp.platform.as_str(),
            roblox_security_level: settings.lsp.roblox_security_level.as_str(),
            binary: acquire::installed_binary(&settings.lsp).map(|path| path.display().to_string()),
            request_timeout_ms: settings.lsp.request_timeout_ms,
        },
        // Memory-only mode loads no sourcemap, so reporting on one would be advice about a file
        // that could not matter here, and the staleness sweep would walk the project for nothing.
        sourcemap: match memory_only {
            true => None,
            false => sourcemap_status(handle, settings).await,
        },
        memories: MemoryStatus {
            count: memories.list()?.len(),
            directory: project::normalize_separators(
                project
                    .memories_dir()
                    .strip_prefix(project.root())
                    .unwrap_or(&project.memories_dir()),
            ),
        },
    })
}

fn settings_status(project: &Project, settings: &Settings) -> SettingsStatus {
    let mut overrides = BTreeMap::new();
    if let (Ok(current), Ok(defaults)) = (
        serde_json::to_value(settings),
        serde_json::to_value(Settings::default()),
    ) {
        collect_overrides(&current, &defaults, "", &mut overrides);
    }

    SettingsStatus {
        settings_file: shown_path(project, &project.settings_path()),
        settings_file_present: project.settings_path().is_file(),
        local_settings_file: shown_path(project, &project.local_settings_path()),
        local_settings_file_present: project.local_settings_path().is_file(),
        overrides,
    }
}

fn shown_path(project: &Project, path: &Path) -> String {
    project::normalize_separators(path.strip_prefix(project.root()).unwrap_or(path))
}

/// Every leaf of `current` that differs from `defaults`, keyed by its dotted path.
///
/// Diffing against the defaults rather than reading the settings files back means an override set
/// in `settings.local.yml`, in an environment, or by a merge is reported the same way as one
/// written in `settings.yml`: what is reported is what is in effect.
fn collect_overrides(
    current: &Value,
    defaults: &Value,
    prefix: &str,
    out: &mut BTreeMap<String, Value>,
) {
    if let (Value::Object(current_map), Value::Object(default_map)) = (current, defaults) {
        for (key, value) in current_map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match default_map.get(key) {
                Some(default) => collect_overrides(value, default, &path, out),
                None => {
                    out.insert(path, value.clone());
                }
            }
        }
        return;
    }
    if current != defaults {
        out.insert(prefix.to_string(), current.clone());
    }
}

async fn sourcemap_status(
    handle: &LanguageServerHandle,
    settings: &Settings,
) -> Option<SourcemapStatus> {
    let relative = settings.lsp.sourcemap.as_ref()?;
    let resolved = handle.project().resolve(relative).ok()?;

    let modified = std::fs::metadata(&resolved)
        .ok()
        .and_then(|metadata| metadata.modified().ok());

    let Some(modified) = modified else {
        return Some(SourcemapStatus {
            relative_path: relative.clone(),
            present: false,
            modified_epoch_seconds: None,
            age_seconds: None,
            watched: settings.lsp.watch_sourcemap,
            stale: None,
            newest_source: None,
            note: Some(MISSING_SOURCEMAP_NOTE.to_string()),
        });
    };

    let newest = newest_source(handle).await;
    let stale = newest
        .as_ref()
        .map(|(_, written)| *written > modified)
        .unwrap_or(false);

    Some(SourcemapStatus {
        relative_path: relative.clone(),
        present: true,
        modified_epoch_seconds: epoch_seconds(modified),
        age_seconds: SystemTime::now()
            .duration_since(modified)
            .ok()
            .map(|elapsed| elapsed.as_secs()),
        watched: settings.lsp.watch_sourcemap,
        stale: Some(stale),
        newest_source: stale
            .then(|| newest.and_then(|(path, _)| handle.project().relativize(&path).ok()))
            .flatten(),
        note: stale.then(|| STALE_SOURCEMAP_NOTE.to_string()),
    })
}

/// The most recently written Luau file in the project, and when it was written.
async fn newest_source(handle: &LanguageServerHandle) -> Option<(PathBuf, SystemTime)> {
    let files = handle.resolve_luau_files(None).await.ok()?;

    // One blocking task for the whole sweep: a stat per file, awaited individually, would suspend
    // and resume the task once per file in the project for an answer nothing waits on twice.
    tokio::task::spawn_blocking(move || {
        files
            .into_iter()
            .filter_map(|path| {
                let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
                Some((path, modified))
            })
            .max_by_key(|(_, modified)| *modified)
    })
    .await
    .ok()
    .flatten()
}

fn epoch_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToolSettings;

    #[test]
    fn only_settings_that_differ_from_the_defaults_are_reported() {
        let settings = Settings {
            tools: ToolSettings {
                max_answer_chars: 42,
                excluded: vec!["find_symbol".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut overrides = BTreeMap::new();
        collect_overrides(
            &serde_json::to_value(&settings).unwrap(),
            &serde_json::to_value(Settings::default()).unwrap(),
            "",
            &mut overrides,
        );

        assert_eq!(overrides["tools.max_answer_chars"], 42);
        assert_eq!(
            overrides["tools.excluded"],
            serde_json::json!(["find_symbol"])
        );
        assert_eq!(overrides.len(), 2, "unexpected overrides: {overrides:?}");
    }

    #[test]
    fn a_default_configuration_reports_no_overrides() {
        let mut overrides = BTreeMap::new();
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        collect_overrides(&defaults, &defaults, "", &mut overrides);
        assert!(overrides.is_empty(), "unexpected overrides: {overrides:?}");
    }
}
