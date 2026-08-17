use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::name_path::NamePathPattern;
use super::protocol::{Diagnostic, Location, Position, Severity, is_low_level_kind};
use super::session::{LanguageServerHandle, Session, ensure_luau_file};
use super::symbols::SymbolNode;
use super::uri;
use crate::bail_hint;
use crate::project::Project;

const NAME_PATH_HINT: &str = "a name path is a symbol name such as \"update\", optionally \
                              qualified with its owners as \"PlayerService:update\"; prefix \"/\" \
                              to anchor it to the top level of the file";

#[derive(Debug, Clone, Serialize)]
pub struct SymbolMatch {
    /// Absent when the location falls outside every symbol in its file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_path: Option<String>,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SymbolMatch>,
}

/// Symbols keyed by the file that defines them, so a path is spelled once per file rather than
/// once per symbol.
pub type SymbolsByFile = BTreeMap<String, Vec<SymbolMatch>>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SymbolSearchResult {
    pub symbols: SymbolsByFile,
    /// True when `max_matches` cut the result set short. Omitted when false.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceMatch {
    pub relative_path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_symbol: Option<String>,
    pub snippet: String,
}

/// Severity is deliberately absent: it is already the key of the map this entry sits under.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticEntry {
    pub line: u32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SeverityGroup {
    /// Diagnostics that fall inside a symbol, keyed by that symbol's name path.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub symbols: BTreeMap<String, Vec<DiagnosticEntry>>,
    /// Diagnostics that belong to the file rather than to any one symbol.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unscoped: Vec<DiagnosticEntry>,
}

pub type GroupedDiagnostics = BTreeMap<String, BTreeMap<String, SeverityGroup>>;

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
            bail_hint!(
                "locate the path with find_file or list_dir, or omit relative_path to search the \
                 whole project";
                "no such file or directory: {relative}"
            );
        }

        let all = self.handle.resolve_luau_files().await?;
        Ok(all
            .into_iter()
            .filter(|path| path.starts_with(&resolved))
            .collect())
    }

    pub async fn find_symbol(&self, request: FindSymbolRequest) -> Result<SymbolSearchResult> {
        let pattern = NamePathPattern::parse(&request.name_path, request.substring_matching);
        if pattern.is_empty() {
            bail_hint!(NAME_PATH_HINT; "name_path must not be empty");
        }

        let session = self.handle.session().await?;
        let files = self
            .candidate_files(request.relative_path.as_deref())
            .await?;
        // Collecting one past the cap is what makes a complete result set distinguishable
        // from a truncated one.
        let probe = request.max_matches.saturating_add(1);
        let mut matches: Vec<(String, SymbolMatch)> = Vec::new();

        for path in files {
            if matches.len() >= probe {
                break;
            }
            let Ok(symbols) = session.document_symbols(&path).await else {
                continue;
            };
            let content = session.ensure_open(&path).await?;
            let relative = self.project().relativize(&path)?;

            let mut found = Vec::new();
            collect_matches(
                &symbols,
                &pattern,
                &request,
                probe - matches.len(),
                &content,
                &mut found,
            );
            matches.extend(found.into_iter().map(|symbol| (relative.clone(), symbol)));
        }

        let truncated = matches.len() > request.max_matches;
        matches.truncate(request.max_matches);
        Ok(SymbolSearchResult {
            symbols: group_by_file(matches),
            truncated,
        })
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
            .map(|symbol| render(symbol, &content, depth, false))
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
            0 => bail_hint!(
                format!(
                    "name paths are case-sensitive; call get_symbols_overview on {relative_path} \
                     to see what it defines. {NAME_PATH_HINT}"
                );
                "no symbol matching {name_path} in {relative_path}"
            ),
            1 => {
                let symbol = found.remove(0);
                let position = symbol.target_position(&content);
                Ok((path, symbol, position))
            }
            count => {
                let names: Vec<&str> = found.iter().map(|node| node.name_path.as_str()).collect();
                bail_hint!(
                    format!(
                        "name one of them in full, for example \"{}\"; same-named siblings are \
                         addressed by index, as in \"{name_path}[0]\"",
                        names[0]
                    );
                    "{name_path} is ambiguous in {relative_path}: {count} matches ({})",
                    names.join(", ")
                )
            }
        }
    }

    pub async fn find_declaration(
        &self,
        name_path: &str,
        relative_path: &str,
        include_body: bool,
    ) -> Result<SymbolsByFile> {
        let session = self.handle.session().await?;
        let (path, symbol, position) = self.locate_one(&session, name_path, relative_path).await?;
        let locations = session.definition(&path, position).await?;

        // A local declared in place has nothing further to point at, so the server answers with
        // nothing. The symbol itself is the correct answer there.
        if locations.is_empty() {
            let content = session.ensure_open(&path).await?;
            let relative = self.project().relativize(&path)?;
            return Ok(SymbolsByFile::from([(
                relative,
                vec![render(&symbol, &content, 0, include_body)],
            )]));
        }

        self.render_locations(&session, locations, include_body)
            .await
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
        for location in locations
            .into_iter()
            .filter(|location| !is_declaration_site(location, &path, position))
            .take(max_results)
        {
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
        include_body: bool,
    ) -> Result<SymbolsByFile> {
        let mut rendered = SymbolsByFile::new();
        for location in locations {
            let Ok(target) = uri::to_path(&location.uri) else {
                continue;
            };
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let symbols = session.document_symbols(&target).await.unwrap_or_default();
            let node = SymbolNode::innermost_at(&symbols, location.range.start);
            let body = if include_body {
                let content = session.ensure_open(&target).await?;
                Some(snippet_around(&content, location.range.start.line))
            } else {
                None
            };

            rendered.entry(relative).or_default().push(SymbolMatch {
                name_path: node.map(|found| found.name_path.clone()),
                kind: node
                    .map(|found| found.kind_label().to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                start_line: location.range.start.line + 1,
                end_line: location.range.end.line + 1,
                detail: node.and_then(|found| found.detail.clone()),
                body,
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
        // The declaring file is already reported at symbol scope; revisiting it at file scope
        // would duplicate every entry.
        let mut visited = std::collections::HashSet::from([path]);

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
                merge_severities(grouped.entry(file).or_default(), severities);
            }
        }
        Ok(grouped)
    }
}

/// luau-lsp reports the declaration even when `includeDeclaration` is false, so drop it here.
fn is_declaration_site(location: &Location, path: &Path, position: Position) -> bool {
    location.range.start == position
        && uri::to_path(&location.uri).is_ok_and(|target| target == path)
}

fn merge_severities(
    into: &mut BTreeMap<String, SeverityGroup>,
    from: BTreeMap<String, SeverityGroup>,
) {
    for (severity, group) in from {
        let target = into.entry(severity).or_default();
        for (symbol, entries) in group.symbols {
            target.symbols.entry(symbol).or_default().extend(entries);
        }
        target.unscoped.extend(group.unscoped);
    }
}

fn group_by_file(matches: Vec<(String, SymbolMatch)>) -> SymbolsByFile {
    let mut grouped = SymbolsByFile::new();
    for (relative_path, symbol) in matches {
        grouped.entry(relative_path).or_default().push(symbol);
    }
    grouped
}

fn collect_matches(
    nodes: &[SymbolNode],
    pattern: &NamePathPattern,
    request: &FindSymbolRequest,
    limit: usize,
    content: &str,
    out: &mut Vec<SymbolMatch>,
) {
    for node in nodes {
        if out.len() >= limit {
            return;
        }
        let kind_allowed = (request.include_kinds.is_empty()
            || request.include_kinds.contains(&node.kind))
            && !request.exclude_kinds.contains(&node.kind);

        if kind_allowed && pattern.matches(&node.ancestors()) {
            out.push(render(node, content, request.depth, request.include_body));
        }
        collect_matches(&node.children, pattern, request, limit, content, out);
    }
}

fn render(node: &SymbolNode, content: &str, depth: u32, include_body: bool) -> SymbolMatch {
    let children = if depth == 0 {
        Vec::new()
    } else {
        node.children
            .iter()
            .filter(|child| !is_low_level_kind(child.kind))
            .map(|child| render(child, content, depth - 1, false))
            .collect()
    };

    SymbolMatch {
        name_path: Some(node.name_path.clone()),
        kind: node.kind_label().to_string(),
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
            .map(|node| node.name_path.clone());

        let bucket = grouped
            .entry(relative_path.to_string())
            .or_default()
            .entry(severity.label().to_string())
            .or_default();

        let entry = DiagnosticEntry {
            line: diagnostic.range.start.line + 1,
            message: diagnostic.message,
            code: diagnostic.code.map(|code| match code {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            }),
        };

        match owner {
            Some(name) => bucket.symbols.entry(name).or_default().push(entry),
            None => bucket.unscoped.push(entry),
        }
    }
    grouped
}

pub fn severity_from_input(value: Option<u32>) -> Result<Severity> {
    match value.unwrap_or(2) {
        1 => Ok(Severity::Error),
        2 => Ok(Severity::Warning),
        3 => Ok(Severity::Information),
        4 => Ok(Severity::Hint),
        other => Err(crate::errors::hinted(
            format!(
                "min_severity must be 1 (error), 2 (warning), 3 (information), or 4 (hint), got \
                 {other}"
            ),
            "omit min_severity to report errors and warnings",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::Range;

    fn position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn range(line: u32) -> Range {
        Range {
            start: position(line, 0),
            end: position(line, 10),
        }
    }

    fn diagnostic(line: u32, message: &str) -> Diagnostic {
        Diagnostic {
            range: range(line),
            severity: Some(1),
            code: None,
            source: None,
            message: message.to_string(),
        }
    }

    fn node(name_path: &str, start_line: u32, end_line: u32) -> SymbolNode {
        SymbolNode {
            name: name_path.rsplit('/').next().unwrap().to_string(),
            name_path: name_path.to_string(),
            kind: 12,
            detail: None,
            range: Range {
                start: position(start_line, 0),
                end: position(end_line, 0),
            },
            selection_range: range(start_line),
            children: Vec::new(),
        }
    }

    #[test]
    fn diagnostics_outside_every_symbol_land_in_unscoped() {
        let symbols = vec![node("PlayerUtils/GetPlayerMaid", 10, 20)];
        let grouped = group_diagnostics(
            [diagnostic(12, "inside"), diagnostic(40, "outside")].into_iter(),
            "src/PlayerUtils.luau",
            &symbols,
        );

        let bucket = &grouped["src/PlayerUtils.luau"]["error"];
        assert_eq!(bucket.symbols["PlayerUtils/GetPlayerMaid"].len(), 1);
        assert_eq!(bucket.unscoped.len(), 1);
        assert_eq!(bucket.unscoped[0].message, "outside");
        assert!(!bucket.symbols.contains_key("<file>"));
    }

    #[test]
    fn merging_severities_keeps_both_sides() {
        let symbols = vec![node("Alpha", 0, 5)];
        let mut into = group_diagnostics(
            [diagnostic(1, "first")].into_iter(),
            "src/Alpha.luau",
            &symbols,
        )
        .remove("src/Alpha.luau")
        .unwrap();

        let from = group_diagnostics(
            [diagnostic(2, "second"), diagnostic(90, "loose")].into_iter(),
            "src/Alpha.luau",
            &symbols,
        )
        .remove("src/Alpha.luau")
        .unwrap();

        merge_severities(&mut into, from);

        assert_eq!(into["error"].symbols["Alpha"].len(), 2);
        assert_eq!(into["error"].unscoped.len(), 1);
    }

    #[test]
    fn only_the_exact_declaration_position_is_filtered() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\project\src\PlayerUtils.luau"
        } else {
            "/project/src/PlayerUtils.luau"
        });
        let other = path.with_file_name("PlotService.luau");
        let anchor = position(103, 21);

        let at = |target: &PathBuf, start: Position| Location {
            uri: uri::from_path(target).unwrap(),
            range: Range {
                start,
                end: position(start.line, start.character + 13),
            },
        };

        assert!(is_declaration_site(&at(&path, anchor), &path, anchor));
        assert!(!is_declaration_site(
            &at(&path, position(154, 8)),
            &path,
            anchor
        ));
        assert!(!is_declaration_site(&at(&other, anchor), &path, anchor));
    }
}
