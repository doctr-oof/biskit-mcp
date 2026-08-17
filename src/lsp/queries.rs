use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use super::name_path::NamePathPattern;
use super::protocol::{Diagnostic, Location, Position, Severity, is_low_level_kind};
use super::session::{LanguageServerHandle, Session, ensure_luau_file};
use super::symbols::SymbolNode;
use super::uri;
use crate::project::Project;

#[derive(Debug, Clone, Serialize)]
pub struct SymbolMatch {
    pub name_path: String,
    pub kind: String,
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SymbolMatch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceMatch {
    pub relative_path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_symbol: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticEntry {
    pub line: u32,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

pub type GroupedDiagnostics =
    BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<DiagnosticEntry>>>>;

pub struct SymbolQuery<'a> {
    pub handle: &'a LanguageServerHandle,
}

#[derive(Debug, Clone)]
pub struct FindSymbolRequest {
    pub name_path: String,
    pub relative_path: Option<String>,
    pub depth: u32,
    pub include_body: bool,
    pub include_kinds: Vec<u32>,
    pub exclude_kinds: Vec<u32>,
    pub substring_matching: bool,
    pub max_matches: usize,
}

impl<'a> SymbolQuery<'a> {
    pub fn new(handle: &'a LanguageServerHandle) -> Self {
        Self { handle }
    }

    fn project(&self) -> &Project {
        self.handle.project()
    }

    async fn candidate_files(&self, relative_path: Option<&str>) -> Result<Vec<PathBuf>> {
        let Some(relative) = relative_path else {
            return self.handle.resolve_luau_files().await;
        };

        let resolved = self.project().resolve(relative)?;
        if resolved.is_file() {
            ensure_luau_file(&resolved)?;
            return Ok(vec![resolved]);
        }
        if !resolved.is_dir() {
            bail!("no such file or directory: {relative}");
        }

        let all = self.handle.resolve_luau_files().await?;
        Ok(all
            .into_iter()
            .filter(|path| path.starts_with(&resolved))
            .collect())
    }

    pub async fn find_symbol(&self, request: FindSymbolRequest) -> Result<Vec<SymbolMatch>> {
        let pattern = NamePathPattern::parse(&request.name_path, request.substring_matching);
        if pattern.is_empty() {
            bail!("name_path must not be empty");
        }

        let session = self.handle.session().await?;
        let files = self
            .candidate_files(request.relative_path.as_deref())
            .await?;
        let mut matches = Vec::new();

        for path in files {
            if matches.len() >= request.max_matches {
                break;
            }
            let Ok(symbols) = session.document_symbols(&path).await else {
                continue;
            };
            let content = session.ensure_open(&path).await?;
            let relative = self.project().relativize(&path)?;

            collect_matches(
                &symbols,
                &pattern,
                &request,
                &relative,
                &content,
                &mut matches,
            );
        }

        matches.truncate(request.max_matches);
        Ok(matches)
    }

    pub async fn symbols_overview(
        &self,
        relative_path: &str,
        depth: u32,
    ) -> Result<Vec<SymbolMatch>> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let symbols = session.document_symbols(&path).await?;
        let content = session.ensure_open(&path).await?;

        // Low-level kinds are pruned from children, not from the top level: a module whose
        // only top-level symbols are variables would otherwise look like an empty file.
        Ok(symbols
            .iter()
            .map(|symbol| render(symbol, relative_path, &content, depth, false))
            .collect())
    }

    /// Resolves a name path to exactly one symbol, erroring when the pattern is ambiguous.
    async fn locate_one(
        &self,
        session: &Session,
        name_path: &str,
        relative_path: &str,
    ) -> Result<(PathBuf, SymbolNode, Position)> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let pattern = NamePathPattern::parse(name_path, false);
        let symbols = session.document_symbols(&path).await?;
        let content = session.ensure_open(&path).await?;

        let mut found = Vec::new();
        for root in &symbols {
            root.walk(&mut |node| {
                if pattern.matches(&node.ancestors()) {
                    found.push(node.clone());
                }
            });
        }

        match found.len() {
            0 => bail!("no symbol matching {name_path} in {relative_path}"),
            1 => {
                let symbol = found.remove(0);
                let position = symbol.target_position(&content);
                Ok((path, symbol, position))
            }
            count => {
                let names: Vec<&str> = found.iter().map(|node| node.name_path.as_str()).collect();
                bail!(
                    "{name_path} is ambiguous in {relative_path}: {count} matches ({}). \
                     Use a more specific name path.",
                    names.join(", ")
                )
            }
        }
    }

    pub async fn find_declaration(
        &self,
        name_path: &str,
        relative_path: &str,
    ) -> Result<Vec<SymbolMatch>> {
        let session = self.handle.session().await?;
        let (path, _, position) = self.locate_one(&session, name_path, relative_path).await?;
        let locations = session.definition(&path, position).await?;
        self.render_locations(&session, locations).await
    }

    pub async fn find_referencing_symbols(
        &self,
        name_path: &str,
        relative_path: &str,
        max_results: usize,
    ) -> Result<Vec<ReferenceMatch>> {
        let session = self.handle.session().await?;
        let (path, _, position) = self.locate_one(&session, name_path, relative_path).await?;
        let locations = session.references(&path, position, false).await?;

        let mut references = Vec::new();
        for location in locations.into_iter().take(max_results) {
            let Ok(target) = uri::to_path(&location.uri) else {
                continue;
            };
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let Ok(content) = session.ensure_open(&target).await else {
                continue;
            };
            let containing = session
                .document_symbols(&target)
                .await
                .ok()
                .and_then(|symbols| {
                    SymbolNode::innermost_at(&symbols, location.range.start)
                        .map(|node| node.name_path.clone())
                });

            references.push(ReferenceMatch {
                relative_path: relative,
                line: location.range.start.line + 1,
                containing_symbol: containing,
                snippet: snippet_around(&content, location.range.start.line),
            });
        }
        Ok(references)
    }

    async fn render_locations(
        &self,
        session: &Session,
        locations: Vec<Location>,
    ) -> Result<Vec<SymbolMatch>> {
        let mut rendered = Vec::new();
        for location in locations {
            let Ok(target) = uri::to_path(&location.uri) else {
                continue;
            };
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let content = session.ensure_open(&target).await?;
            let symbols = session.document_symbols(&target).await.unwrap_or_default();
            let node = SymbolNode::innermost_at(&symbols, location.range.start);

            rendered.push(SymbolMatch {
                name_path: node
                    .map(|found| found.name_path.clone())
                    .unwrap_or_else(|| "<file>".to_string()),
                kind: node
                    .map(|found| found.kind_label().to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                relative_path: relative,
                start_line: location.range.start.line + 1,
                end_line: location.range.end.line + 1,
                detail: node.and_then(|found| found.detail.clone()),
                body: Some(snippet_around(&content, location.range.start.line)),
                children: Vec::new(),
            });
        }
        Ok(rendered)
    }

    pub async fn file_diagnostics(
        &self,
        relative_path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
        min_severity: Severity,
    ) -> Result<GroupedDiagnostics> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let diagnostics = session.diagnostics(&path).await?;
        let symbols = session.document_symbols(&path).await.unwrap_or_default();
        let relative = self.project().relativize(&path)?;

        let filtered = diagnostics.into_iter().filter(|diagnostic| {
            let severity = Severity::from_code(diagnostic.severity);
            if severity > min_severity {
                return false;
            }
            match (start_line, end_line) {
                (None, None) => true,
                (from, to) => diagnostic.range.overlaps_lines(
                    from.unwrap_or(1).saturating_sub(1),
                    to.map(|line| line.saturating_sub(1)).unwrap_or(u32::MAX),
                ),
            }
        });

        Ok(group_diagnostics(filtered, &relative, &symbols))
    }

    pub async fn symbol_diagnostics(
        &self,
        name_path: &str,
        relative_path: &str,
        check_references: bool,
        min_severity: Severity,
    ) -> Result<GroupedDiagnostics> {
        let session = self.handle.session().await?;
        let (path, symbol, position) = self.locate_one(&session, name_path, relative_path).await?;

        let mut grouped = self
            .file_diagnostics(
                relative_path,
                Some(symbol.range.start.line + 1),
                Some(symbol.range.end.line + 1),
                min_severity,
            )
            .await?;

        if !check_references {
            return Ok(grouped);
        }

        let locations = session.references(&path, position, false).await?;
        let mut visited = std::collections::HashSet::new();

        for location in locations {
            let Ok(target) = uri::to_path(&location.uri) else {
                continue;
            };
            if !visited.insert(target.clone()) {
                continue;
            }
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let Ok(referencing) = self
                .file_diagnostics(&relative, None, None, min_severity)
                .await
            else {
                continue;
            };
            for (file, severities) in referencing {
                grouped.entry(file).or_default().extend(severities);
            }
        }
        Ok(grouped)
    }
}

fn collect_matches(
    nodes: &[SymbolNode],
    pattern: &NamePathPattern,
    request: &FindSymbolRequest,
    relative_path: &str,
    content: &str,
    out: &mut Vec<SymbolMatch>,
) {
    for node in nodes {
        if out.len() >= request.max_matches {
            return;
        }
        let kind_allowed = (request.include_kinds.is_empty()
            || request.include_kinds.contains(&node.kind))
            && !request.exclude_kinds.contains(&node.kind);

        if kind_allowed && pattern.matches(&node.ancestors()) {
            out.push(render(
                node,
                relative_path,
                content,
                request.depth,
                request.include_body,
            ));
        }
        collect_matches(
            &node.children,
            pattern,
            request,
            relative_path,
            content,
            out,
        );
    }
}

fn render(
    node: &SymbolNode,
    relative_path: &str,
    content: &str,
    depth: u32,
    include_body: bool,
) -> SymbolMatch {
    let children = if depth == 0 {
        Vec::new()
    } else {
        node.children
            .iter()
            .filter(|child| !is_low_level_kind(child.kind))
            .map(|child| render(child, relative_path, content, depth - 1, false))
            .collect()
    };

    SymbolMatch {
        name_path: node.name_path.clone(),
        kind: node.kind_label().to_string(),
        relative_path: relative_path.to_string(),
        start_line: node.range.start.line + 1,
        end_line: node.range.end.line + 1,
        detail: node.detail.clone(),
        body: include_body.then(|| extract_body(content, node)),
        children,
    }
}

fn extract_body(content: &str, node: &SymbolNode) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = node.range.start.line as usize;
    let end = (node.range.end.line as usize).min(lines.len().saturating_sub(1));
    if start > end {
        return String::new();
    }
    lines[start..=end].join("\n")
}

fn snippet_around(content: &str, line: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let index = (line as usize).min(lines.len().saturating_sub(1));
    let start = index.saturating_sub(1);
    let end = (index + 1).min(lines.len().saturating_sub(1));
    lines[start..=end].join("\n")
}

fn group_diagnostics(
    diagnostics: impl Iterator<Item = Diagnostic>,
    relative_path: &str,
    symbols: &[SymbolNode],
) -> GroupedDiagnostics {
    let mut grouped: GroupedDiagnostics = BTreeMap::new();

    for diagnostic in diagnostics {
        let severity = Severity::from_code(diagnostic.severity);
        let owner = SymbolNode::innermost_at(symbols, diagnostic.range.start)
            .map(|node| node.name_path.clone())
            .unwrap_or_else(|| "<file>".to_string());

        grouped
            .entry(relative_path.to_string())
            .or_default()
            .entry(severity.label().to_string())
            .or_default()
            .entry(owner)
            .or_default()
            .push(DiagnosticEntry {
                line: diagnostic.range.start.line + 1,
                severity: severity.label().to_string(),
                message: diagnostic.message,
                code: diagnostic.code.map(|code| match code {
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                }),
            });
    }
    grouped
}

pub fn severity_from_input(value: Option<u32>) -> Result<Severity> {
    match value.unwrap_or(2) {
        1 => Ok(Severity::Error),
        2 => Ok(Severity::Warning),
        3 => Ok(Severity::Information),
        4 => Ok(Severity::Hint),
        other => Err(anyhow!(
            "min_severity must be 1 (error), 2 (warning), 3 (information), or 4 (hint), got {other}"
        )),
    }
}
