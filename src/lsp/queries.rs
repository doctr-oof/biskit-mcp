use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use super::client;
use super::name_path::NamePathPattern;
use super::protocol::{Diagnostic, Location, Position, Severity, is_low_level_kind};
use super::session::{LanguageServerHandle, Session, ensure_luau_file};
use super::symbols::SymbolNode;
use super::uri;
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::project::Project;

const NAME_PATH_HINT: &str = "a name path is a symbol name such as \"update\", optionally \
                              qualified with its owners as \"PlayerService:update\"; prefix \"/\" \
                              to anchor it to the top level of the file";

const SCAN_ABORTED: &str = "the project scan stopped early because the language server stopped \
                            answering; restart it with restart_language_server";

/// Lines either side of a declaration reported with `include_body`. A declaration whose own symbol
/// could not be resolved has only its line to show, so one line of context earns its place there.
const DECLARATION_CONTEXT_LINES: usize = 1;

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
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_symbol: Option<String>,
    pub snippet: String,
}

/// References keyed by the file they appear in, on the same reasoning as `SymbolsByFile`.
pub type ReferencesByFile = BTreeMap<String, Vec<ReferenceMatch>>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferenceSearchResult {
    pub references: ReferencesByFile,
    /// True when `max_reference_matches` cut the result set short. Omitted when false.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
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

/// What a rendered symbol carries beyond its name, kind, and line range. `detail` is the language
/// server's type signature, which is long enough to be worth asking for rather than assuming.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    pub depth: u32,
    pub include_body: bool,
    pub include_detail: bool,
}

#[derive(Debug, Clone)]
pub struct FindSymbolRequest {
    pub name_path: String,
    pub relative_path: Option<String>,
    pub depth: u32,
    pub include_body: bool,
    pub include_detail: bool,
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
            return self.handle.resolve_luau_files(None).await;
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

        // Walking the named subtree rather than walking the project and filtering afterwards.
        self.handle.resolve_luau_files(Some(&resolved)).await
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
        let files = prefilter_by_literal(files, pattern.literal_filter()).await?;

        // Collecting one past the cap is what makes a complete result set distinguishable
        // from a truncated one.
        let probe = request.max_matches.saturating_add(1);
        let mut matches: Vec<(String, SymbolMatch)> = Vec::new();

        for path in files {
            if matches.len() >= probe {
                break;
            }
            let (symbols, content) = match session.document_symbols(&path).await {
                Ok(found) => found,
                // One file failing to parse is worth stepping over. A server that has stopped
                // answering is not: every remaining file would burn a full request timeout,
                // turning a thirty second failure into an hours long one.
                Err(error) if client::is_unavailable(&error) => {
                    return Err(error).context(SCAN_ABORTED);
                }
                Err(_) => continue,
            };
            let relative = self.project().relativize(&path)?;
            let lines = LineIndex::new(&content);

            let mut found = Vec::new();
            collect_matches(
                &symbols,
                &pattern,
                &request,
                probe - matches.len(),
                &lines,
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
        include_detail: bool,
    ) -> Result<Vec<SymbolMatch>> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let (symbols, content) = session.document_symbols(&path).await?;
        let lines = LineIndex::new(&content);

        let options = RenderOptions {
            depth,
            include_body: false,
            include_detail,
        };

        // Low-level kinds are pruned from children, not from the top level: a module whose
        // only top-level symbols are variables would otherwise look like an empty file.
        Ok(symbols
            .iter()
            .map(|symbol| render(symbol, &lines, options))
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
        let (symbols, content) = session.document_symbols(&path).await?;

        let mut found = Vec::new();
        for root in &symbols {
            root.walk(&mut |node| {
                if pattern.matches(&node.name_path) {
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
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let session = self.handle.session().await?;
        let (path, symbol, position) = self.locate_one(&session, name_path, relative_path).await?;
        let locations = session.definition(&path, position).await?;

        // A local declared in place has nothing further to point at, so the server answers with
        // nothing. The symbol itself is the correct answer there.
        if locations.is_empty() {
            let content = session.ensure_open(&path).await?.content;
            let relative = self.project().relativize(&path)?;
            let options = RenderOptions {
                depth: 0,
                include_body,
                include_detail,
            };
            return Ok(SymbolsByFile::from([(
                relative,
                vec![render(&symbol, &LineIndex::new(&content), options)],
            )]));
        }

        self.render_locations(&session, locations, include_body, include_detail)
            .await
    }

    pub async fn find_referencing_symbols(
        &self,
        name_path: &str,
        relative_path: &str,
        max_results: usize,
        context_lines: usize,
    ) -> Result<ReferenceSearchResult> {
        let session = self.handle.session().await?;
        let (path, _, position) = self.locate_one(&session, name_path, relative_path).await?;
        let locations = session.references(&path, position, false).await?;

        // Collecting one past the cap is what makes a complete result set distinguishable
        // from a truncated one.
        let probe = max_results.saturating_add(1);
        let mut references: Vec<(String, ReferenceMatch)> = Vec::new();

        let wanted: Vec<Location> = locations
            .into_iter()
            .filter(|location| !is_declaration_site(location, &path, position))
            .collect();

        // Forty references spread over five files are five files' worth of information. Reading
        // and re-requesting the symbol tree once per reference asked the server for the same file
        // as many times as it happened to appear.
        'files: for (target, group) in group_locations_by_file(wanted) {
            if references.len() >= probe {
                break;
            }
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let Ok((symbols, content)) = session.document_symbols(&target).await else {
                continue;
            };
            let lines = LineIndex::new(&content);

            for location in group {
                if references.len() >= probe {
                    break 'files;
                }
                let containing = SymbolNode::innermost_at(&symbols, location.range.start)
                    .map(|node| node.name_path.clone());

                references.push((
                    relative.clone(),
                    ReferenceMatch {
                        line: location.range.start.line + 1,
                        containing_symbol: containing,
                        snippet: snippet_around(&lines, location.range.start.line, context_lines),
                    },
                ));
            }
        }

        let truncated = references.len() > max_results;
        references.truncate(max_results);
        Ok(ReferenceSearchResult {
            references: group_by_file(references),
            truncated,
        })
    }

    async fn render_locations(
        &self,
        session: &Session,
        locations: Vec<Location>,
        include_body: bool,
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let mut rendered = SymbolsByFile::new();
        for (target, group) in group_locations_by_file(locations) {
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let (symbols, content) = session
                .document_symbols(&target)
                .await
                .unwrap_or_else(|_| (Vec::new(), Arc::from("")));
            let lines = LineIndex::new(&content);

            for location in group {
                let node = SymbolNode::innermost_at(&symbols, location.range.start);
                rendered
                    .entry(relative.clone())
                    .or_default()
                    .push(SymbolMatch {
                        name_path: node.map(|found| found.name_path.clone()),
                        kind: node
                            .map(|found| found.kind_label().to_string())
                            .unwrap_or_else(|| "Unknown".to_string()),
                        start_line: location.range.start.line + 1,
                        end_line: location.range.end.line + 1,
                        detail: include_detail
                            .then(|| node.and_then(|found| found.detail.clone()))
                            .flatten(),
                        body: include_body.then(|| {
                            snippet_around(
                                &lines,
                                location.range.start.line,
                                DECLARATION_CONTEXT_LINES,
                            )
                        }),
                        children: Vec::new(),
                    });
            }
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
        let symbols = session
            .document_symbols(&path)
            .await
            .map(|(symbols, _)| symbols)
            .unwrap_or_default();
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

fn group_by_file<T>(matches: Vec<(String, T)>) -> BTreeMap<String, Vec<T>> {
    let mut grouped: BTreeMap<String, Vec<T>> = BTreeMap::new();
    for (relative_path, item) in matches {
        grouped.entry(relative_path).or_default().push(item);
    }
    grouped
}

/// Groups locations by the file they point into, keeping the order in which each file was first
/// seen so a truncated result set is still the first N in the server's own ordering.
fn group_locations_by_file(locations: Vec<Location>) -> Vec<(PathBuf, Vec<Location>)> {
    let mut order: Vec<(PathBuf, Vec<Location>)> = Vec::new();
    let mut seen: std::collections::HashMap<PathBuf, usize> = std::collections::HashMap::new();

    for location in locations {
        let Ok(target) = uri::to_path(&location.uri) else {
            continue;
        };
        match seen.get(&target) {
            Some(index) => order[*index].1.push(location),
            None => {
                seen.insert(target.clone(), order.len());
                order.push((target, vec![location]));
            }
        }
    }
    order
}

/// Drops candidate files whose bytes never spell `needle`.
///
/// A symbol cannot be defined in a file that does not contain its name, and reading a file and
/// searching it for a literal is orders of magnitude cheaper than a `documentSymbol` round trip
/// through a single stdio pipe. For the common exploratory query, which matches nothing, this is
/// the difference between one request per file in the project and none.
///
/// A file that cannot be read is kept, so the language server reports the problem rather than the
/// file quietly vanishing from the result set. A single candidate is never filtered: a query
/// naming one file should behave exactly as it did before.
pub async fn prefilter_by_literal(
    files: Vec<PathBuf>,
    needle: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let Some(needle) = needle.filter(|_| files.len() > 1) else {
        return Ok(files);
    };
    let needle = needle.to_string();

    let filtered = tokio::task::spawn_blocking(move || {
        let finder = memchr::memmem::Finder::new(needle.as_bytes());
        let mut kept = Vec::new();
        for path in files {
            match std::fs::read(&path) {
                Ok(bytes) if finder.find(&bytes).is_none() => continue,
                _ => kept.push(path),
            }
        }
        kept
    })
    .await
    .map_err(|error| anyhow::anyhow!("candidate pre-filter panicked: {error}"))?;

    Ok(filtered)
}

fn collect_matches(
    nodes: &[SymbolNode],
    pattern: &NamePathPattern,
    request: &FindSymbolRequest,
    limit: usize,
    lines: &LineIndex<'_>,
    out: &mut Vec<SymbolMatch>,
) {
    for node in nodes {
        if out.len() >= limit {
            return;
        }
        let kind_allowed = (request.include_kinds.is_empty()
            || request.include_kinds.contains(&node.kind))
            && !request.exclude_kinds.contains(&node.kind);

        if kind_allowed && pattern.matches(&node.name_path) {
            out.push(render(
                node,
                lines,
                RenderOptions {
                    depth: request.depth,
                    include_body: request.include_body,
                    include_detail: request.include_detail,
                },
            ));
        }
        collect_matches(&node.children, pattern, request, limit, lines, out);
    }
}

/// Renders a symbol that sits at the top of a result, named by its full name path.
fn render(node: &SymbolNode, lines: &LineIndex<'_>, options: RenderOptions) -> SymbolMatch {
    render_node(node, lines, options, true)
}

/// Renders a nested symbol, named by its own leaf segment. The ancestry is already spelled out by
/// the chain of parents it sits under, so repeating it would cost the caller the prefix on every
/// child. Join a child's name to its parent's name path with `/` to address it.
fn render_child(node: &SymbolNode, lines: &LineIndex<'_>, options: RenderOptions) -> SymbolMatch {
    let options = RenderOptions {
        include_body: false,
        ..options
    };
    render_node(node, lines, options, false)
}

fn render_node(
    node: &SymbolNode,
    lines: &LineIndex<'_>,
    options: RenderOptions,
    full_name_path: bool,
) -> SymbolMatch {
    let children = if options.depth == 0 {
        Vec::new()
    } else {
        let nested = RenderOptions {
            depth: options.depth - 1,
            ..options
        };
        node.children
            .iter()
            .filter(|child| !is_low_level_kind(child.kind))
            .map(|child| render_child(child, lines, nested))
            .collect()
    };

    let name = if full_name_path {
        node.name_path.clone()
    } else {
        node.name.clone()
    };

    SymbolMatch {
        name_path: Some(name),
        kind: node.kind_label().to_string(),
        start_line: node.range.start.line + 1,
        end_line: node.range.end.line + 1,
        detail: options
            .include_detail
            .then(|| node.detail.clone())
            .flatten(),
        body: options.include_body.then(|| extract_body(lines, node)),
        children,
    }
}

fn extract_body(lines: &LineIndex<'_>, node: &SymbolNode) -> String {
    lines
        .text(node.range.start.line as usize, node.range.end.line as usize)
        .into_owned()
}

/// The line at `line`, widened by `context` lines on each side.
fn snippet_around(lines: &LineIndex<'_>, line: u32, context: usize) -> String {
    let index = lines.clamp_line(line as usize);
    lines
        .text(index.saturating_sub(context), index + context)
        .into_owned()
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

    fn at(path: &Path, line: u32) -> Location {
        Location {
            uri: uri::from_path(path).unwrap(),
            range: Range {
                start: position(line, 0),
                end: position(line, 8),
            },
        }
    }

    #[test]
    fn locations_group_by_file_in_first_seen_order() {
        let root = PathBuf::from(if cfg!(windows) {
            r"C:\project\src"
        } else {
            "/project/src"
        });
        let alpha = root.join("Alpha.luau");
        let beta = root.join("Beta.luau");

        let grouped = group_locations_by_file(vec![
            at(&beta, 4),
            at(&alpha, 1),
            at(&beta, 9),
            at(&alpha, 2),
            at(&beta, 12),
        ]);

        assert_eq!(grouped.len(), 2, "each file appears once");
        assert_eq!(grouped[0].0, beta, "first file seen stays first");
        assert_eq!(
            grouped[0]
                .1
                .iter()
                .map(|l| l.range.start.line)
                .collect::<Vec<_>>(),
            vec![4, 9, 12],
            "order within a file is preserved"
        );
        assert_eq!(grouped[1].0, alpha);
        assert_eq!(
            grouped[1]
                .1
                .iter()
                .map(|l| l.range.start.line)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn locations_with_unreadable_uris_are_dropped_from_the_grouping() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\project\src\Alpha.luau"
        } else {
            "/project/src/Alpha.luau"
        });
        let bad = Location {
            uri: "https://example.com/Alpha.luau".to_string(),
            range: range(3),
        };

        let grouped = group_locations_by_file(vec![bad, at(&path, 3)]);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, path);
    }

    #[test]
    fn the_prefilter_keeps_only_files_that_spell_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let mentions = dir.path().join("Mentions.luau");
        let silent = dir.path().join("Silent.luau");
        let missing = dir.path().join("Missing.luau");
        std::fs::write(&mentions, "function Utils:GetPlayerMaid()\nend\n").unwrap();
        std::fs::write(&silent, "local unrelated = 1\n").unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let files = vec![mentions.clone(), silent, missing.clone()];

        let kept = runtime
            .block_on(prefilter_by_literal(files.clone(), Some("GetPlayerMaid")))
            .unwrap();
        assert_eq!(
            kept,
            vec![mentions, missing],
            "a file that cannot be read is kept so the server reports it"
        );

        // No usable literal, and a lone candidate, both leave the set untouched.
        assert_eq!(
            runtime
                .block_on(prefilter_by_literal(files.clone(), None))
                .unwrap(),
            files
        );
        let single = vec![files[1].clone()];
        assert_eq!(
            runtime
                .block_on(prefilter_by_literal(single.clone(), Some("GetPlayerMaid")))
                .unwrap(),
            single,
            "a query naming one file behaves as it did before the filter existed"
        );
    }

    #[test]
    fn the_prefilter_survives_a_substring_query() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("Kept.luau");
        let dropped = dir.path().join("Dropped.luau");
        std::fs::write(&kept, "function Utils:GetPlayerMaid()\nend\n").unwrap();
        std::fs::write(&dropped, "function Utils:Reset()\nend\n").unwrap();

        let pattern = NamePathPattern::parse("PlayerMaid", true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let survivors = runtime
            .block_on(prefilter_by_literal(
                vec![kept.clone(), dropped],
                pattern.literal_filter(),
            ))
            .unwrap();
        assert_eq!(survivors, vec![kept]);
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
