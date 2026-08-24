use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::config::Settings;
use crate::lsp::acquire;
use crate::lsp::session::LanguageServerHandle;
use crate::memory::MemoryStore;
use crate::project::{self, Project};
use crate::roblox::RobloxIndex;

/// Everything a confused caller needs to tell "no matches" from "nothing is running".
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_require: Option<SharedRequireStatus>,
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
    /// Absent when nothing has been downloaded yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    pub request_timeout_ms: u64,
}

/// Where the carpenter fork's `shared("Name")` require stands for this project.
#[derive(Debug, Clone, Serialize)]
pub struct SharedRequireStatus {
    /// Whether the require graph counts `shared("Name")` calls as dependency edges.
    pub graph_edges: bool,
    /// One of `supported`, `unsupported`, or `unknown`.
    pub language_server: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// True when a Luau file has been written since the sourcemap was generated.
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

const SHARED_REQUIRE_SKEW_NOTE: &str = "the require graph follows shared(\"Name\") calls but the \
                                        pinned language server release predates support for them, \
                                        so diagnostics will report as errors the same calls the \
                                        graph reports as edges. Raise lsp.version.";

const SHARED_REQUIRE_OFF_NOTE: &str = "project.shared_require is off, so the require graph does \
                                       not count shared(\"Name\") calls as dependencies. The \
                                       language server still resolves them, so hover, diagnostics \
                                       and go-to-definition disagree with get_require_graph and \
                                       get_module_context here.";

pub async fn collect(
    handle: &LanguageServerHandle,
    roblox: &RobloxIndex,
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
        sourcemap: match memory_only {
            true => None,
            false => sourcemap_status(handle, roblox, settings).await,
        },
        shared_require: match memory_only {
            true => None,
            false => Some(shared_require_status(settings)),
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

fn shared_require_status(settings: &Settings) -> SharedRequireStatus {
    let graph_edges = settings.project.shared_require;
    let language_server = match parsed_version(&settings.lsp.version) {
        _ if settings.lsp.repository != crate::config::DEFAULT_LSP_REPOSITORY => "unknown",
        Some(version) if version >= crate::config::FIRST_SHARED_REQUIRE_VERSION => "supported",
        Some(_) => "unsupported",
        None => "unknown",
    };

    let note = match (graph_edges, language_server) {
        (true, "unsupported") => Some(SHARED_REQUIRE_SKEW_NOTE.to_string()),
        (false, "supported") => Some(SHARED_REQUIRE_OFF_NOTE.to_string()),
        _ => None,
    };

    SharedRequireStatus {
        graph_edges,
        language_server,
        note,
    }
}

fn parsed_version(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u32>().ok();
    let parsed = (next()?, next()?, next()?);
    parts.next().is_none().then_some(parsed)
}

async fn sourcemap_status(
    handle: &LanguageServerHandle,
    roblox: &RobloxIndex,
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

    let newest = roblox.newest_source().await;
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
    fn a_pinned_release_too_old_for_shared_require_is_called_out() {
        let settings = Settings {
            lsp: crate::config::LspSettings {
                version: "v0.1.17".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let status = shared_require_status(&settings);
        assert!(status.graph_edges);
        assert_eq!(status.language_server, "unsupported");
        assert!(status.note.is_some());
    }

    #[test]
    fn the_default_pin_resolves_shared_require_and_needs_no_note() {
        let status = shared_require_status(&Settings::default());
        assert!(status.graph_edges);
        assert_eq!(status.language_server, "supported");
        assert!(status.note.is_none(), "unexpected note: {:?}", status.note);
    }

    #[test]
    fn turning_the_graph_half_off_is_reported_as_the_disagreement_it_is() {
        let settings = Settings {
            project: crate::config::ProjectSettings {
                shared_require: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let status = shared_require_status(&settings);
        assert!(!status.graph_edges);
        assert_eq!(status.language_server, "supported");
        assert!(
            status
                .note
                .as_deref()
                .is_some_and(|note| note.contains("still resolves them")),
            "unexpected note: {:?}",
            status.note
        );
    }

    #[test]
    fn a_version_or_repository_biskit_cannot_judge_is_not_guessed_at() {
        let unparseable = Settings {
            lsp: crate::config::LspSettings {
                version: "nightly".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            shared_require_status(&unparseable).language_server,
            "unknown"
        );

        let forked = Settings {
            lsp: crate::config::LspSettings {
                repository: "someone/their-own-fork".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(shared_require_status(&forked).language_server, "unknown");
    }

    #[test]
    fn a_default_configuration_reports_no_overrides() {
        let mut overrides = BTreeMap::new();
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        collect_overrides(&defaults, &defaults, "", &mut overrides);
        assert!(overrides.is_empty(), "unexpected overrides: {overrides:?}");
    }
}
