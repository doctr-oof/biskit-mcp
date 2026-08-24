use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use super::RobloxIndex;
use super::requires::{Direction, ReachedModule, UnresolvedEntry};
use super::sourcemap::SourcemapReference;
use crate::bail_hint;
use crate::lsp::protocol::Severity;
use crate::lsp::queries::{ModuleApi, SymbolQuery};
use crate::lsp::session::LanguageServerHandle;

const SERVER_ROOTS: [&str; 4] = [
    "ServerScriptService",
    "ServerStorage",
    "RobloxServerStorage",
    "ServerPackages",
];

const CLIENT_ROOTS: [&str; 4] = [
    "StarterPlayer",
    "StarterGui",
    "StarterPack",
    "StarterCharacterScripts",
];

const SHARED_ROOTS: [&str; 6] = [
    "ReplicatedStorage",
    "ReplicatedFirst",
    "Workspace",
    "Lighting",
    "SoundService",
    "TextChatService",
];

/// Everything an agent works out about an unfamiliar module before it can safely touch it.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleContext {
    pub relative_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    /// The top-level service the module sits under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Where the code runs: `server`, `client`, `shared`, or `unknown`.
    pub role: &'static str,
    /// What it requires, directly.
    pub requires: Vec<ReachedModule>,
    /// What requires it, directly.
    pub required_by: Vec<ReachedModule>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_requires: Vec<UnresolvedEntry>,
    /// Its public surface, when it is a ModuleScript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<ModuleApi>,
    /// How many diagnostics it currently carries, by severity.
    pub diagnostics: BTreeMap<String, usize>,
    pub sourcemap: SourcemapReference,
    /// Anything that made the answer less complete than it looks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Answers, in one call, the four to six questions an agent asks when it opens a module it has not seen before.
pub async fn module_context(
    index: &RobloxIndex,
    handle: &LanguageServerHandle,
    relative_path: &str,
    max_entries: usize,
) -> Result<ModuleContext> {
    let path = index.project().resolve(relative_path)?;
    crate::lsp::session::ensure_luau_file(&path)?;
    if !path.is_file() {
        bail_hint!(
            "locate it with find_file using the mask \"*.luau\", or list the directory with \
             list_dir";
            "no such file: {relative_path}"
        );
    }
    let relative = index.project().relativize(&path)?;

    let sourcemap = index.sourcemap().await?;
    let node = sourcemap.nodes_for_file(&relative).first().copied();
    let mut notes = Vec::new();

    if node.is_none() {
        notes.push(NOT_SYNCED_NOTE.to_string());
    }

    let graph = index.require_graph().await?;
    let (requires, required_by, unresolved_requires) = match graph.find(&relative) {
        Some(module) => {
            let (requires, _) = graph.walk(module, Direction::Dependencies, 1, max_entries);
            let (required_by, _) = graph.walk(module, Direction::Dependents, 1, max_entries);
            let unresolved = graph
                .module(module)
                .unresolved
                .iter()
                .take(max_entries)
                .map(|entry| UnresolvedEntry {
                    relative_path: relative.clone(),
                    line: entry.line,
                    expression: entry.expression.clone(),
                    reason: entry.reason.clone(),
                })
                .collect();
            (requires, required_by, unresolved)
        }
        None => {
            notes.push(NOT_SCANNED_NOTE.to_string());
            (Vec::new(), Vec::new(), Vec::new())
        }
    };

    let query = SymbolQuery::new(handle);
    let api = match query.module_api(&relative, max_entries).await {
        Ok(api) => Some(api),
        Err(error) => {
            notes.push(format!("the public surface could not be read: {error}"));
            None
        }
    };

    let diagnostics = match handle.session().await {
        Ok(session) => match session.diagnostics(&path).await {
            Ok(found) => count_by_severity(found),
            Err(error) => {
                notes.push(format!("diagnostics could not be read: {error}"));
                BTreeMap::new()
            }
        },
        Err(error) => {
            notes.push(format!("diagnostics could not be read: {error}"));
            BTreeMap::new()
        }
    };

    let service = node
        .and_then(|node| sourcemap.service_of(node))
        .map(str::to_string);
    Ok(ModuleContext {
        relative_path: relative,
        instance_path: node.map(|node| sourcemap.node(node).instance_path.clone()),
        class_name: node.map(|node| sourcemap.node(node).class_name.clone()),
        role: role_of(service.as_deref()),
        service,
        requires,
        required_by,
        unresolved_requires,
        api,
        diagnostics,
        sourcemap: sourcemap.reference(),
        notes,
    })
}

fn count_by_severity(
    diagnostics: Vec<crate::lsp::protocol::Diagnostic>,
) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for diagnostic in diagnostics {
        let severity = Severity::from_code(diagnostic.severity);
        *counts.entry(severity.label().to_string()).or_default() += 1;
    }
    counts
}

fn role_of(service: Option<&str>) -> &'static str {
    let Some(service) = service else {
        return "unknown";
    };
    if SERVER_ROOTS.contains(&service) {
        return "server";
    }
    if CLIENT_ROOTS.contains(&service) {
        return "client";
    }
    if SHARED_ROOTS.contains(&service) {
        return "shared";
    }
    "unknown"
}

const NOT_SYNCED_NOTE: &str = "this file is not in the sourcemap, so it is not synced into the \
                               game by the rojo project and editing it changes nothing at \
                               runtime, unless the sourcemap is simply out of date.";

const NOT_SCANNED_NOTE: &str = "this file is outside the set Biskit scans, so its requires and \
                                its dependents are not known. Check project.ignored_paths.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unambiguous_services_decide_where_code_runs() {
        assert_eq!(role_of(Some("ServerScriptService")), "server");
        assert_eq!(role_of(Some("StarterPlayer")), "client");
        assert_eq!(role_of(Some("ReplicatedStorage")), "shared");
        assert_eq!(role_of(Some("Chat")), "unknown");
        assert_eq!(role_of(None), "unknown");
    }
}
