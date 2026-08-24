use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use super::client;
use super::name_path::{NamePathPattern, strip_overload_suffix};
use super::protocol::{
    Diagnostic, Documentation, InlayHint, Location, Position, Range, Severity, SignatureHelp,
    inlay_hint_kind_label, is_low_level_kind,
};
use super::session::{LanguageServerHandle, Session, ensure_luau_file};
use super::symbols::{SymbolNode, find_identifier, is_identifier_byte};
use super::uri;
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::project::Project;

const NAME_PATH_HINT: &str = "a name path is a symbol name such as \"update\", optionally \
                              qualified with its owners as \"PlayerService:update\"; prefix \"/\" \
                              to anchor it to the top level of the file";

const SCAN_ABORTED: &str = "the project scan stopped early because the language server stopped \
                            answering; restart it with restart_language_server";

const DECLARATION_CONTEXT_LINES: usize = 1;

const POINT_HINT: &str = "name the symbol with name_path, or give the line and column of a use of \
                          it; line and column are 1-based, as every Biskit result reports them";

const MAX_DOCUMENTATION_CHARS: usize = 4_000;

const TYPE_DEFINITION_HINT: &str = "aim line and column at the type's own name: in \
                                    `local config: PlayerConfig`, at `PlayerConfig` rather than \
                                    at `config`. A value with no written annotation has no type \
                                    declaration to find, and explain_symbol reports what it \
                                    resolved to instead.";

const NO_HINTS_NOTE: &str = "no hints in this range: luau-lsp emits a hint only where a type or an \
                             argument name is not already written out. The luau-lsp.inlayHints.* \
                             keys under lsp.server_settings control which kinds are emitted.";

const NO_SIGNATURES_NOTE: &str = "the language server answered with no signatures. Signature help \
                                  is only available from inside the parentheses of a call, so aim \
                                  line and column at an argument position rather than at the \
                                  function's declaration.";

const MAX_DETAIL_HOVERS: usize = 200;

const DETAIL_CAPPED_NOTE: &str = "detail was filled for the first symbols only: one hover request \
                                  per symbol is spent resolving a signature, and this answer hit \
                                  the ceiling. Narrow the answer with relative_path or a lower \
                                  depth, or ask explain_symbol about the symbols still missing a \
                                  detail.";

const SELF_REFERENCE_NOTE: &str = "references marked resolved_by \"text\" were found by scanning \
                                   the declaring file for `self:` and `self.` uses. luau-lsp types \
                                   the implicit self of a colon-declared method as a fresh generic \
                                   rather than as the owner, so it reports no reference for those \
                                   call sites. They are matched on the symbol's own name, so \
                                   confirm the receiver before treating one as a call site.";

const TEXT_RESOLUTION: &str = "text";

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
    /// Children the low-level kind filter dropped.
    #[serde(skip_serializing_if = "crate::json::is_zero")]
    pub omitted_children: usize,
    /// Where to aim a hover to fill `detail`.
    #[serde(skip)]
    pub hover_at: Option<Position>,
}

/// Symbols keyed by the file that defines them.
pub type SymbolsByFile = BTreeMap<String, Vec<SymbolMatch>>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SymbolSearchResult {
    pub symbols: SymbolsByFile,
    /// True when `max_matches` cut the result set short.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
    /// Set only when the detail budget ran out before every symbol carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The symbols of one file, in the shape `get_symbols_overview` answers with.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SymbolOverviewResult {
    pub symbols: Vec<SymbolMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceMatch {
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_symbol: Option<String>,
    pub snippet: String,
    /// Set to "text" on a reference the `self:` scan recovered rather than the language server reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<&'static str>,
}

/// References keyed by the file they appear in, on the same reasoning as `SymbolsByFile`.
pub type ReferencesByFile = BTreeMap<String, Vec<ReferenceMatch>>;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferenceSearchResult {
    pub references: ReferencesByFile,
    /// True when `max_reference_matches` cut the result set short.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
    /// Set only when the answer carries a reference the text scan recovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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

/// What the language server resolved a symbol to, in the form an agent reads.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolExplanation {
    pub relative_path: String,
    pub line: u32,
    pub column: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The resolved type, taken from the code half of the hover.
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlayHintEntry {
    pub line: u32,
    pub column: u32,
    /// Text the editor would draw at that position, such as `: number`.
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlayHintResult {
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub hints: Vec<InlayHintEntry>,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
    /// Set only when the hint list is empty, where an empty list on its own reads as a failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignatureParameter {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignatureEntry {
    pub label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<SignatureParameter>,
    /// Index into `parameters` of the argument the position sits on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_parameter: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignatureHelpResult {
    pub relative_path: String,
    pub line: u32,
    pub column: u32,
    pub signatures: Vec<SignatureEntry>,
    /// Index into `signatures` of the overload the server considers active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_signature: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One entry of a module's public surface.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleExport {
    pub name: String,
    pub kind: String,
    pub line: u32,
    /// The language server's signature for it, which is the type the caller will be handed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An `export type` declaration, quoted as written.
#[derive(Debug, Clone, Serialize)]
pub struct ExportedType {
    pub name: String,
    pub line: u32,
    pub declaration: String,
}

/// What a ModuleScript hands back, and nothing else.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleApi {
    pub relative_path: String,
    /// The returned expression as written, so a module that returns something unusual still says what it returns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// One of `table`, `function`, `expression`, or `none`.
    pub return_kind: &'static str,
    pub exports: Vec<ModuleExport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<ExportedType>,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
    /// Why the surface is empty or partial, on the paths where it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where a request points inside a file.
#[derive(Debug, Clone, Copy)]
pub enum SymbolPoint<'a> {
    NamePath(&'a str),
    LineColumn { line: u32, column: u32 },
}

impl<'a> SymbolPoint<'a> {
    /// Line and column are 1-based, as every Biskit result reports them.
    pub fn parse(
        name_path: Option<&'a str>,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<Self> {
        let name_path = name_path
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty());

        match (name_path, line) {
            (Some(_), Some(_)) => bail_hint!(
                POINT_HINT;
                "pass either name_path or line, not both: they name different positions"
            ),
            (Some(name_path), None) => Ok(Self::NamePath(name_path)),
            (None, Some(0)) => {
                bail_hint!(POINT_HINT; "line is 1-based, so 0 names no line")
            }
            (None, Some(line)) => Ok(Self::LineColumn {
                line,
                column: column.unwrap_or(1).max(1),
            }),
            (None, None) => bail_hint!(POINT_HINT; "no position given"),
        }
    }
}

struct ResolvedPoint {
    path: PathBuf,
    relative_path: String,
    position: Position,
    symbol: Option<SymbolNode>,
}

pub struct SymbolQuery<'a> {
    pub handle: &'a LanguageServerHandle,
}

/// What a rendered symbol carries beyond its name, kind, and line range.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    pub depth: u32,
    pub include_body: bool,
    pub include_detail: bool,
    /// Descend into locals declared inside a body, which are pruned by default as noise.
    pub include_locals: bool,
}

#[derive(Debug, Clone)]
pub struct FindSymbolRequest {
    pub name_path: String,
    pub relative_path: Option<String>,
    pub depth: u32,
    pub include_body: bool,
    pub include_detail: bool,
    pub include_locals: bool,
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

        let probe = request.max_matches.saturating_add(1);
        let mut matches: Vec<(String, SymbolMatch)> = Vec::new();
        let mut budget = MAX_DETAIL_HOVERS;
        let mut capped = false;

        for path in files {
            if matches.len() >= probe {
                break;
            }
            let (symbols, content) = match self.handle.document_symbols(&session, &path).await {
                Ok(found) => found,
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
            if request.include_detail {
                capped |= attach_details(&session, &path, &mut found, &mut budget).await;
            }
            matches.extend(found.into_iter().map(|symbol| (relative.clone(), symbol)));
        }

        let truncated = matches.len() > request.max_matches;
        matches.truncate(request.max_matches);
        Ok(SymbolSearchResult {
            symbols: group_by_file(matches),
            truncated,
            note: capped.then(|| DETAIL_CAPPED_NOTE.to_string()),
        })
    }

    pub async fn symbols_overview(
        &self,
        relative_path: &str,
        depth: u32,
        include_detail: bool,
        include_locals: bool,
    ) -> Result<SymbolOverviewResult> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let (symbols, content) = self.handle.document_symbols(&session, &path).await?;
        let lines = LineIndex::new(&content);

        let options = RenderOptions {
            depth,
            include_body: false,
            include_detail,
            include_locals,
        };

        let mut rendered: Vec<SymbolMatch> = symbols
            .iter()
            .map(|symbol| render(symbol, &lines, options))
            .collect();

        let mut budget = MAX_DETAIL_HOVERS;
        let capped =
            include_detail && attach_details(&session, &path, &mut rendered, &mut budget).await;

        Ok(SymbolOverviewResult {
            symbols: rendered,
            note: capped.then(|| DETAIL_CAPPED_NOTE.to_string()),
        })
    }

    async fn locate_one(
        &self,
        session: &Session,
        name_path: &str,
        relative_path: &str,
    ) -> Result<(PathBuf, SymbolNode, Position)> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let pattern = NamePathPattern::parse(name_path, false);
        let (symbols, content) = self.handle.document_symbols(session, &path).await?;

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

    async fn locate_point(
        &self,
        session: &Session,
        point: SymbolPoint<'_>,
        relative_path: &str,
    ) -> Result<ResolvedPoint> {
        if let SymbolPoint::NamePath(name_path) = point {
            let (path, symbol, position) =
                self.locate_one(session, name_path, relative_path).await?;
            let relative = self.project().relativize(&path)?;
            return Ok(ResolvedPoint {
                path,
                relative_path: relative,
                position,
                symbol: Some(symbol),
            });
        }

        let SymbolPoint::LineColumn { line, column } = point else {
            unreachable!("the name path case returned above");
        };

        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let file = session.ensure_open(&path).await?;
        let lines = LineIndex::new(&file.content);
        if line as usize > lines.len() {
            bail_hint!(
                "line numbers are 1-based; get_symbols_overview shows where the file's symbols \
                 start and end";
                "line {line} is past the end of {relative_path}, which has {} lines",
                lines.len()
            );
        }

        let position = Position {
            line: line - 1,
            character: column - 1,
        };

        let symbols = self
            .handle
            .document_symbols(session, &path)
            .await
            .map(|(symbols, _)| symbols)
            .unwrap_or_default();

        Ok(ResolvedPoint {
            relative_path: self.project().relativize(&path)?,
            path,
            position,
            symbol: SymbolNode::innermost_at(&symbols, position).cloned(),
        })
    }

    /// The resolved type of whatever sits at `point`, plus its doc comment when asked for.
    pub async fn explain_symbol(
        &self,
        point: SymbolPoint<'_>,
        relative_path: &str,
        include_documentation: bool,
    ) -> Result<SymbolExplanation> {
        let session = self.handle.session().await?;
        let resolved = self.locate_point(&session, point, relative_path).await?;

        let markdown = session
            .hover(&resolved.path, resolved.position)
            .await?
            .map(|hover| hover.contents.into_markdown())
            .unwrap_or_default();

        let (signature, documentation) = split_hover(&markdown);
        if signature.is_empty() && documentation.is_empty() {
            bail_hint!(
                POINT_HINT;
                "the language server has no hover information at {relative_path}:{}:{}",
                resolved.position.line + 1,
                resolved.position.character + 1
            );
        }

        Ok(SymbolExplanation {
            signature: strip_unbound_generics(&signature),
            relative_path: resolved.relative_path,
            line: resolved.position.line + 1,
            column: resolved.position.character + 1,
            name_path: resolved
                .symbol
                .as_ref()
                .map(|symbol| symbol.name_path.clone()),
            kind: resolved
                .symbol
                .as_ref()
                .map(|symbol| symbol.kind_label().to_string()),
            documentation: include_documentation
                .then(|| cap_documentation(documentation))
                .filter(|text| !text.is_empty()),
        })
    }

    /// Where the *type* of a symbol is declared.
    pub async fn type_definition(
        &self,
        point: SymbolPoint<'_>,
        relative_path: &str,
        include_body: bool,
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let session = self.handle.session().await?;
        let resolved = self.locate_point(&session, point, relative_path).await?;

        let locations = match session
            .type_definition(&resolved.path, resolved.position)
            .await
        {
            Ok(locations) => locations,
            Err(error) if client::is_unsupported(&error) => bail_hint!(
                "use find_declaration for the value's own declaration";
                "this luau-lsp build does not implement textDocument/typeDefinition"
            ),
            Err(error) => return Err(error),
        };

        if locations.is_empty() {
            bail_hint!(
                TYPE_DEFINITION_HINT;
                "the language server knows no type declaration at {relative_path}:{}:{}",
                resolved.position.line + 1,
                resolved.position.character + 1
            );
        }

        self.render_locations(&session, locations, include_body, include_detail)
            .await
    }

    /// The inferred types luau-lsp would draw inline over a line range.
    pub async fn inlay_hints(
        &self,
        relative_path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
        max_hints: usize,
    ) -> Result<InlayHintResult> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let file = session.ensure_open(&path).await?;
        let lines = LineIndex::new(&file.content);
        let relative = self.project().relativize(&path)?;

        if lines.is_empty() {
            return Ok(InlayHintResult {
                relative_path: relative,
                start_line: 0,
                end_line: 0,
                hints: Vec::new(),
                truncated: false,
                note: Some(format!("{relative_path} is empty")),
            });
        }

        let last = lines.len() as u32 - 1;
        let from = start_line
            .map_or(0, |line| line.saturating_sub(1))
            .min(last);
        let to = end_line
            .map_or(last, |line| line.saturating_sub(1))
            .clamp(from, last);
        let range = Range {
            start: Position {
                line: from,
                character: 0,
            },
            end: Position {
                line: to,
                character: lines.slice(to as usize, to as usize).chars().count() as u32,
            },
        };

        let hints = match session.inlay_hints(&path, range).await {
            Ok(hints) => hints,
            Err(error) if client::is_unsupported(&error) => bail_hint!(
                "find_symbol with include_detail reports declared signatures, and explain_symbol \
                 reports the resolved type of one symbol";
                "this luau-lsp build does not implement textDocument/inlayHint"
            ),
            Err(error) => return Err(error),
        };

        let mut in_range: Vec<InlayHint> = hints
            .into_iter()
            .filter(|hint| (from..=to).contains(&hint.position.line))
            .collect();
        in_range.sort_by_key(|hint| hint.position);

        let truncated = in_range.len() > max_hints;
        let entries: Vec<InlayHintEntry> = in_range
            .into_iter()
            .take(max_hints)
            .map(|hint| InlayHintEntry {
                line: hint.position.line + 1,
                column: hint.position.character + 1,
                kind: inlay_hint_kind_label(hint.kind),
                label: hint.label.into_text().trim().to_string(),
            })
            .collect();

        Ok(InlayHintResult {
            relative_path: relative,
            start_line: from + 1,
            end_line: to + 1,
            note: entries.is_empty().then(|| NO_HINTS_NOTE.to_string()),
            hints: entries,
            truncated,
        })
    }

    /// The parameters of the call at `point`, without reading the callee.
    pub async fn signature_help(
        &self,
        point: SymbolPoint<'_>,
        relative_path: &str,
        include_documentation: bool,
    ) -> Result<SignatureHelpResult> {
        let session = self.handle.session().await?;
        let resolved = self.locate_point(&session, point, relative_path).await?;

        let help = match session
            .signature_help(&resolved.path, resolved.position)
            .await
        {
            Ok(help) => help,
            Err(error) if client::is_unsupported(&error) => bail_hint!(
                "explain_symbol reports the callee's resolved type, which carries the same \
                 parameter list";
                "this luau-lsp build does not implement textDocument/signatureHelp"
            ),
            Err(error) => return Err(error),
        };
        let help = help.unwrap_or(SignatureHelp {
            signatures: Vec::new(),
            active_signature: None,
            active_parameter: None,
        });

        let fallback_parameter = help.active_parameter;
        let signatures: Vec<SignatureEntry> = help
            .signatures
            .into_iter()
            .map(|signature| SignatureEntry {
                parameters: signature
                    .parameters
                    .iter()
                    .map(|parameter| SignatureParameter {
                        label: parameter.label.resolve(&signature.label),
                        documentation: include_documentation
                            .then(|| {
                                parameter
                                    .documentation
                                    .clone()
                                    .map(Documentation::into_text)
                            })
                            .flatten()
                            .filter(|text| !text.is_empty()),
                    })
                    .collect(),
                active_parameter: signature.active_parameter.or(fallback_parameter),
                documentation: include_documentation
                    .then(|| signature.documentation.map(Documentation::into_text))
                    .flatten()
                    .map(cap_documentation)
                    .filter(|text| !text.is_empty()),
                label: signature.label,
            })
            .collect();

        Ok(SignatureHelpResult {
            relative_path: resolved.relative_path,
            line: resolved.position.line + 1,
            column: resolved.position.character + 1,
            active_signature: help.active_signature,
            note: signatures
                .is_empty()
                .then(|| NO_SIGNATURES_NOTE.to_string()),
            signatures,
        })
    }

    /// The public surface of a ModuleScript: what its returned value exposes, plus the types it exports, and none of the body either was implemented in.
    pub async fn module_api(&self, relative_path: &str, max_exports: usize) -> Result<ModuleApi> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let (symbols, content) = self.handle.document_symbols(&session, &path).await?;
        let relative = self.project().relativize(&path)?;

        let blanked = crate::roblox::requires::blank_comments(&content);
        let types = exported_types(&content, &blanked, max_exports);

        let Some((line, expression)) = top_level_return(&blanked) else {
            return Ok(ModuleApi {
                relative_path: relative,
                returns: None,
                return_kind: "none",
                exports: Vec::new(),
                types,
                truncated: false,
                note: Some(NOT_A_MODULE_NOTE.to_string()),
            });
        };

        let (name, kind_from_text) = returned_name(&expression);
        let owner = name
            .as_deref()
            .and_then(|name| top_level_symbol(&symbols, name));

        let mut exports: Vec<ModuleExport> = owner
            .map(|node| {
                node.children
                    .iter()
                    .map(|child| ModuleExport {
                        name: child.name.clone(),
                        kind: child.kind_label().to_string(),
                        line: child.range.start.line + 1,
                        detail: child.detail.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        exports.sort_by_key(|export| export.line);

        let truncated = exports.len() > max_exports;
        exports.truncate(max_exports);

        let return_kind = match (kind_from_text, owner) {
            (Some(kind), _) => kind,
            (None, Some(node)) if !node.children.is_empty() => "table",
            (None, _) => "expression",
        };

        let note = match (owner, return_kind) {
            (_, "table_literal") => Some(format!("{TABLE_LITERAL_NOTE} It starts on line {line}.")),
            (_, "function") => Some(FUNCTION_RETURN_NOTE.to_string()),
            (None, _) => Some(format!(
                "{UNTRACED_RETURN_NOTE} It returns `{expression}` on line {line}."
            )),
            (Some(node), _) if node.children.is_empty() => {
                Some(format!("{EMPTY_TABLE_NOTE} {}", node.name_path))
            }
            _ => None,
        };

        Ok(ModuleApi {
            relative_path: relative,
            returns: (return_kind != "table_literal").then_some(expression),
            return_kind,
            exports,
            types,
            truncated,
            note,
        })
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

        if locations.is_empty() {
            let content = session.ensure_open(&path).await?.content;
            let relative = self.project().relativize(&path)?;
            let options = RenderOptions {
                depth: 0,
                include_body,
                include_detail,
                include_locals: false,
            };
            let mut rendered = vec![render(&symbol, &LineIndex::new(&content), options)];
            if include_detail {
                let mut budget = MAX_DETAIL_HOVERS;
                attach_details(&session, &path, &mut rendered, &mut budget).await;
            }
            return Ok(SymbolsByFile::from([(relative, rendered)]));
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
        let (path, symbol, position) = self.locate_one(&session, name_path, relative_path).await?;
        self.references_at(
            &session,
            &path,
            position,
            Some(&symbol),
            max_results,
            context_lines,
        )
        .await
    }

    async fn references_at(
        &self,
        session: &Session,
        path: &Path,
        position: Position,
        symbol: Option<&SymbolNode>,
        max_results: usize,
        context_lines: usize,
    ) -> Result<ReferenceSearchResult> {
        let locations = session.references(path, position, false).await?;

        let probe = max_results.saturating_add(1);
        let mut references: Vec<(String, ReferenceMatch)> = Vec::new();

        let mut wanted: Vec<Location> = locations
            .into_iter()
            .filter(|location| !is_declaration_site(location, path, position))
            .collect();

        let reported: HashSet<u32> = wanted
            .iter()
            .filter(|location| uri::to_path(&location.uri).is_ok_and(|target| target == path))
            .map(|location| location.range.start.line)
            .collect();
        let recovered = self
            .self_receiver_locations(session, path, symbol, &reported)
            .await?;
        let recovered_lines: HashSet<u32> = recovered
            .iter()
            .map(|location| location.range.start.line)
            .collect();
        wanted.extend(recovered);

        let mut grouped = group_locations_by_file(wanted);
        if !recovered_lines.is_empty()
            && let Some((_, group)) = grouped
                .iter_mut()
                .find(|(target, _)| target.as_path() == path)
        {
            group.sort_by_key(|location| {
                (location.range.start.line, location.range.start.character)
            });
        }

        'files: for (target, group) in grouped {
            if references.len() >= probe {
                break;
            }
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let Ok((symbols, content)) = self.handle.document_symbols(session, &target).await
            else {
                continue;
            };
            let lines = LineIndex::new(&content);
            let declaring = target.as_path() == path;

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
                        resolved_by: (declaring
                            && recovered_lines.contains(&location.range.start.line))
                        .then_some(TEXT_RESOLUTION),
                    },
                ));
            }
        }

        let truncated = references.len() > max_results;
        references.truncate(max_results);
        let recovered_kept = references
            .iter()
            .any(|(_, reference)| reference.resolved_by.is_some());
        Ok(ReferenceSearchResult {
            references: group_by_file(references),
            truncated,
            note: recovered_kept.then(|| SELF_REFERENCE_NOTE.to_string()),
        })
    }

    async fn self_receiver_locations(
        &self,
        session: &Session,
        path: &Path,
        symbol: Option<&SymbolNode>,
        reported_lines: &HashSet<u32>,
    ) -> Result<Vec<Location>> {
        let Some(symbol) = symbol.filter(|node| node.name_path.contains('/')) else {
            return Ok(Vec::new());
        };

        let content = session.ensure_open(path).await?.content;
        let blanked = crate::roblox::requires::blank_comments(&content);
        let uri = uri::from_path(path)?;

        Ok(
            self_receiver_positions(&blanked, strip_overload_suffix(&symbol.name))
                .into_iter()
                .filter(|position| !reported_lines.contains(&position.line))
                .map(|position| Location {
                    uri: uri.clone(),
                    range: Range {
                        start: position,
                        end: position,
                    },
                })
                .collect(),
        )
    }

    async fn render_locations(
        &self,
        session: &Session,
        locations: Vec<Location>,
        include_body: bool,
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let mut rendered = SymbolsByFile::new();
        let mut budget = MAX_DETAIL_HOVERS;
        for (target, group) in group_locations_by_file(locations) {
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let (symbols, content) = self
                .handle
                .document_symbols(session, &target)
                .await
                .unwrap_or_else(|_| (Vec::new(), Arc::from("")));
            let lines = LineIndex::new(&content);

            let mut here: Vec<SymbolMatch> = Vec::new();
            for location in group {
                let node = SymbolNode::innermost_at(&symbols, location.range.start);
                here.push(SymbolMatch {
                    name_path: node.map(|found| found.name_path.clone()),
                    kind: node
                        .map(|found| found.kind_label().to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    start_line: location.range.start.line + 1,
                    end_line: location.range.end.line + 1,
                    detail: None,
                    body: include_body.then(|| {
                        snippet_around(&lines, location.range.start.line, DECLARATION_CONTEXT_LINES)
                    }),
                    children: Vec::new(),
                    omitted_children: 0,
                    hover_at: include_detail.then_some(location.range.start),
                });
            }

            if include_detail {
                attach_details(session, &target, &mut here, &mut budget).await;
            }
            rendered.entry(relative).or_default().extend(here);
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
        let symbols = self
            .handle
            .document_symbols(&session, &path)
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

fn top_level_return(blanked: &str) -> Option<(u32, String)> {
    let mut found = None;
    for (index, line) in blanked.lines().enumerate() {
        let Some(rest) = line.strip_prefix("return") else {
            continue;
        };
        if rest.is_empty()
            || rest
                .chars()
                .next()
                .is_some_and(|first| first.is_whitespace() || first == '(')
        {
            found = Some((index as u32 + 1, rest.trim().to_string()));
        }
    }
    found
}

fn returned_name(expression: &str) -> (Option<String>, Option<&'static str>) {
    let trimmed = expression.trim();
    if trimmed.starts_with("function") {
        return (None, Some("function"));
    }
    if trimmed.starts_with('{') {
        return (None, Some("table_literal"));
    }
    if let Some(rest) = trimmed.strip_prefix("setmetatable(") {
        let first = rest.split(',').next().unwrap_or_default().trim();
        return (identifier(first), None);
    }
    (identifier(trimmed), None)
}

fn identifier(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_end_matches(')');
    (!trimmed.is_empty()
        && trimmed
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
        && !trimmed.starts_with(|first: char| first.is_ascii_digit()))
    .then(|| trimmed.to_string())
}

fn top_level_symbol<'a>(symbols: &'a [SymbolNode], name: &str) -> Option<&'a SymbolNode> {
    symbols
        .iter()
        .find(|node| node.name == name || node.name_path == name)
}

fn export_type_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"(?m)^[ \t]*export[ \t]+type[ \t]+([A-Za-z_][A-Za-z0-9_]*)")
            .expect("the export type pattern is a literal")
    })
}

const MAX_TYPE_DECLARATION_LINES: usize = 40;

fn exported_types(source: &str, blanked: &str, limit: usize) -> Vec<ExportedType> {
    let original: Vec<&str> = source.lines().collect();
    let scanned: Vec<&str> = blanked.lines().collect();
    let index = LineIndex::new(blanked);

    let mut found = Vec::new();
    for capture in export_type_pattern().captures_iter(blanked) {
        if found.len() >= limit {
            break;
        }
        let whole = capture.get(0).expect("group zero always matches");
        let start = index.line_of(whole.start());

        let mut depth = 0isize;
        let mut declaration: Vec<&str> = Vec::new();
        for offset in 0..MAX_TYPE_DECLARATION_LINES {
            let Some(line) = scanned.get(start + offset) else {
                break;
            };
            declaration.push(original.get(start + offset).copied().unwrap_or(line));
            depth += bracket_delta(line);
            if depth <= 0 && !continues(line) {
                break;
            }
        }

        found.push(ExportedType {
            name: capture[1].to_string(),
            line: start as u32 + 1,
            declaration: declaration.join("\n").trim_end().to_string(),
        });
    }
    found
}

fn bracket_delta(line: &str) -> isize {
    line.chars().fold(0, |depth, character| match character {
        '{' | '(' | '[' => depth + 1,
        '}' | ')' | ']' => depth - 1,
        _ => depth,
    })
}

fn continues(line: &str) -> bool {
    matches!(
        line.trim_end().chars().next_back(),
        Some('=' | '|' | '&' | ',' | '<')
    )
}

const NOT_A_MODULE_NOTE: &str = "this file returns nothing, so it is a Script or a LocalScript \
                                 rather than a ModuleScript and has no public surface. Use \
                                 get_symbols_overview to see what it defines.";

const UNTRACED_RETURN_NOTE: &str = "the returned value is not a table declared in this file, so \
                                    there is no surface to list.";

const TABLE_LITERAL_NOTE: &str = "this module returns a table written inline in the return \
                                  statement rather than a named one, which the symbol tree does \
                                  not describe. Read the return statement itself, or use \
                                  explain_symbol on it for the type it resolves to.";

const FUNCTION_RETURN_NOTE: &str = "this module returns a function rather than a table, so \
                                    calling it is its whole surface. Use explain_symbol on the \
                                    return statement for the function's signature.";

const EMPTY_TABLE_NOTE: &str = "the returned table has no members the language server can see in \
                                this file. Members assigned through another name, or by a loop, \
                                are invisible here. The table is";

fn split_hover(markdown: &str) -> (String, String) {
    let mut signature: Vec<&str> = Vec::new();
    let mut documentation: Vec<&str> = Vec::new();
    let mut inside_fence = false;
    let mut fenced = false;

    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            inside_fence = !inside_fence;
            fenced = true;
            continue;
        }
        if inside_fence {
            signature.push(line);
        } else if !is_horizontal_rule(line) {
            documentation.push(line);
        }
    }

    let prose = documentation.join("\n").trim().to_string();
    if !fenced {
        return (prose, String::new());
    }
    (signature.join("\n").trim().to_string(), prose)
}

fn strip_unbound_generics(signature: &str) -> String {
    let Some((open, close)) = generic_list(signature) else {
        return signature.to_string();
    };

    let parameters = split_generics(&signature[open + 1..close]);
    if parameters.is_empty() {
        return signature.to_string();
    }

    let outside = format!("{}{}", &signature[..open], &signature[close + 1..]);
    let kept: Vec<&str> = parameters
        .iter()
        .copied()
        .filter(|parameter| !is_unbound_generic(parameter, &outside))
        .collect();

    if kept.len() == parameters.len() {
        return signature.to_string();
    }
    if kept.is_empty() {
        return outside;
    }
    format!(
        "{}<{}>{}",
        &signature[..open],
        kept.join(", "),
        &signature[close + 1..]
    )
}

fn generic_list(signature: &str) -> Option<(usize, usize)> {
    let limit = signature.find('(').unwrap_or(signature.len());
    let bytes = signature.as_bytes();
    let open = signature[..limit].find('<')?;
    if open == 0 || !is_identifier_byte(bytes[open - 1]) {
        return None;
    }

    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().skip(open).take(limit - open) {
        match byte {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_generics(list: &str) -> Vec<&str> {
    let mut parameters = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;

    for (index, character) in list.char_indices() {
        match character {
            '<' | '(' | '{' | '[' => depth += 1,
            '>' | ')' | '}' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parameters.push(list[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }

    let last = list[start..].trim();
    if !last.is_empty() {
        parameters.push(last);
    }
    parameters.retain(|parameter| !parameter.is_empty());
    parameters
}

fn is_unbound_generic(parameter: &str, outside: &str) -> bool {
    let name = parameter.strip_suffix("...").unwrap_or(parameter);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    if !name.bytes().all(is_identifier_byte) {
        return false;
    }
    find_identifier(outside, name).is_none()
}

fn self_receiver_positions(blanked: &str, name: &str) -> Vec<Position> {
    if name.is_empty() {
        return Vec::new();
    }

    let lines = LineIndex::new(blanked);
    let bytes = blanked.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;

    while let Some(offset) = blanked[from..].find("self") {
        let start = from + offset;
        from = start + "self".len();

        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            continue;
        }
        if !matches!(bytes.get(from), Some(b':') | Some(b'.')) {
            continue;
        }

        let leaf = from + 1;
        if !blanked[leaf..].starts_with(name) {
            continue;
        }
        if bytes
            .get(leaf + name.len())
            .is_some_and(|byte| is_identifier_byte(*byte))
        {
            continue;
        }

        let (line, column) = lines.position_of(leaf);
        found.push(Position {
            line: line as u32,
            character: column as u32,
        });
    }
    found
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    matches!(first, '-' | '_' | '*')
        && trimmed.chars().count() >= 3
        && trimmed.chars().all(|character| character == first)
}

fn cap_documentation(text: String) -> String {
    if text.chars().count() <= MAX_DOCUMENTATION_CHARS {
        return text;
    }
    let mut capped: String = text.chars().take(MAX_DOCUMENTATION_CHARS).collect();
    capped.push_str("\n\n[documentation truncated]");
    capped
}

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
                    include_locals: request.include_locals,
                },
            ));
        }
        collect_matches(&node.children, pattern, request, limit, lines, out);
    }
}

fn render(node: &SymbolNode, lines: &LineIndex<'_>, options: RenderOptions) -> SymbolMatch {
    render_node(node, lines, options, true)
}

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
    let (children, omitted_children) = if options.depth == 0 {
        (Vec::new(), 0)
    } else {
        let nested = RenderOptions {
            depth: options.depth - 1,
            ..options
        };
        let visible: Vec<&SymbolNode> = node
            .children
            .iter()
            .filter(|child| {
                options.include_locals || child.member || !is_low_level_kind(child.kind)
            })
            .collect();
        let omitted = node.children.len() - visible.len();
        (
            visible
                .into_iter()
                .map(|child| render_child(child, lines, nested))
                .collect(),
            omitted,
        )
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
        detail: None,
        body: options.include_body.then(|| extract_body(lines, node)),
        children,
        omitted_children,
        hover_at: options
            .include_detail
            .then(|| node.target_position(lines.content())),
    }
}

async fn attach_details(
    session: &Session,
    path: &Path,
    matches: &mut [SymbolMatch],
    budget: &mut usize,
) -> bool {
    let mut pending: Vec<&mut SymbolMatch> = matches.iter_mut().rev().collect();
    let mut capped = false;

    while let Some(entry) = pending.pop() {
        let SymbolMatch {
            detail,
            hover_at,
            children,
            ..
        } = entry;
        pending.extend(children.iter_mut().rev());

        let Some(position) = hover_at.take() else {
            continue;
        };
        if *budget == 0 {
            capped = true;
            continue;
        }
        *budget -= 1;

        let Ok(Some(hover)) = session.hover(path, position).await else {
            continue;
        };
        let (signature, _) = split_hover(&hover.contents.into_markdown());
        if !signature.is_empty() {
            *detail = Some(strip_unbound_generics(&signature));
        }
    }
    capped
}

fn extract_body(lines: &LineIndex<'_>, node: &SymbolNode) -> String {
    lines
        .text(node.range.start.line as usize, node.range.end.line as usize)
        .into_owned()
}

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

    #[test]
    fn only_a_return_at_the_top_level_is_the_modules_own() {
        let source = "local Combat = {}\n\
                      function Combat.hit()\n\
                      \treturn true\n\
                      end\n\
                      return Combat\n";
        let (line, expression) = top_level_return(source).unwrap();
        assert_eq!(line, 5);
        assert_eq!(expression, "Combat");
    }

    #[test]
    fn a_file_that_returns_nothing_has_no_return_to_find() {
        assert!(top_level_return("print(\"hello\")\n").is_none());
    }

    #[test]
    fn the_returned_value_is_named_through_the_wrappers_modules_use() {
        assert_eq!(returned_name("Combat").0.as_deref(), Some("Combat"));
        assert_eq!(
            returned_name("setmetatable(Class, Class)").0.as_deref(),
            Some("Class")
        );
        assert_eq!(returned_name("function(a) end").1, Some("function"));
        assert_eq!(returned_name("{").1, Some("table_literal"));
    }

    #[test]
    fn a_returned_expression_that_is_not_a_bare_name_names_nothing() {
        assert!(returned_name("Combat.new").0.is_none());
        assert!(returned_name("require(script.Other)").0.is_none());
    }

    #[test]
    fn an_exported_type_is_quoted_to_the_end_of_its_declaration() {
        let source = "export type Config = {\n\tName: string,\n\tCount: number,\n}\n\
                      export type Id = string\n";
        let found = exported_types(source, source, 10);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Config");
        assert_eq!(found[0].line, 1);
        assert!(
            found[0].declaration.ends_with('}'),
            "{:?}",
            found[0].declaration
        );
        assert_eq!(found[1].declaration, "export type Id = string");
    }

    #[test]
    fn a_commented_out_type_is_not_an_export() {
        let source = "-- export type Old = string\nexport type New = number\n";
        let blanked = crate::roblox::requires::blank_comments(source);
        let found = exported_types(source, &blanked, 10);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "New");
        assert_eq!(found[0].line, 2);
    }

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
            member: false,
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
    fn hover_splits_on_its_fences_and_drops_the_separator() {
        let (signature, documentation) = split_hover(
            "```luau\nfunction PlayerService:update(dt: number): ()\n```\n\n---\n\nUpdates the \
             player.\n",
        );
        assert_eq!(signature, "function PlayerService:update(dt: number): ()");
        assert_eq!(documentation, "Updates the player.");
    }

    #[test]
    fn hover_without_a_fence_is_all_signature() {
        let (signature, documentation) = split_hover("Instance?");
        assert_eq!(signature, "Instance?");
        assert!(documentation.is_empty());

        assert_eq!(split_hover(""), (String::new(), String::new()));
    }

    #[test]
    fn an_inferred_generic_is_dropped_and_a_used_one_is_kept() {
        assert_eq!(
            strip_unbound_generics("function PlayerUtils:GetPlayerMaid<a>(player: Player): Maid"),
            "function PlayerUtils:GetPlayerMaid(player: Player): Maid"
        );
        assert_eq!(
            strip_unbound_generics("function Table.find<T>(haystack: {T}, needle: T): number?"),
            "function Table.find<T>(haystack: {T}, needle: T): number?"
        );
        assert_eq!(
            strip_unbound_generics("function Signal:Fire<T, a>(value: T): ()"),
            "function Signal:Fire<T>(value: T): ()"
        );
        assert_eq!(
            strip_unbound_generics("function Maid:Give<a...>(): ()"),
            "function Maid:Give(): ()"
        );
    }

    #[test]
    fn a_signature_with_nothing_to_strip_is_returned_as_written() {
        for signature in [
            "local cachedInfo: {\n    Id: number\n}",
            "function Config.load(path: string): Config",
            "Instance?",
            "",
            "function Pack:Add<T = string>(value: T): ()",
        ] {
            assert_eq!(strip_unbound_generics(signature), signature);
        }
    }

    #[test]
    fn angle_brackets_that_are_not_a_generic_list_are_left_alone() {
        for signature in [
            "function step(count: number): ()",
            "local compare: (number, number) -> boolean",
            "local handler: <a>(a) -> a",
        ] {
            assert_eq!(strip_unbound_generics(signature), signature);
        }
    }

    #[test]
    fn self_calls_are_found_and_bare_ones_are_not() {
        let source = "function PlayerUtils:GetPlayerMaid(player)\n\
                      end\n\
                      local function run()\n\
                          return self:GetPlayerMaid(player)\n\
                      end\n\
                      local cached = self.GetPlayerMaidCache\n\
                      local other = GetPlayerMaid(player)\n\
                      local nested = myself:GetPlayerMaid(player)\n";

        let found = self_receiver_positions(source, "GetPlayerMaid");
        assert_eq!(found.len(), 1, "found {found:?}");
        assert_eq!(found[0].line, 3);
        assert_eq!(
            &source.lines().nth(3).unwrap()[found[0].character as usize..],
            "GetPlayerMaid(player)"
        );
    }

    #[test]
    fn a_commented_out_self_call_is_not_a_reference() {
        let blanked = crate::roblox::requires::blank_comments(
            "-- self:GetPlayerMaid(player)\nreturn self:GetPlayerMaid(player)\n",
        );
        let found = self_receiver_positions(&blanked, "GetPlayerMaid");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn a_body_local_is_pruned_by_default_and_counted() {
        let mut owner = node("PlayerUtils/FetchUserInfo", 10, 20);
        let mut local = node("PlayerUtils/FetchUserInfo/cachedInfo", 12, 12);
        local.kind = 13;
        owner.children.push(local);

        let content = "";
        let lines = LineIndex::new(content);
        let pruned = render(
            &owner,
            &lines,
            RenderOptions {
                depth: 2,
                ..RenderOptions::default()
            },
        );
        assert!(pruned.children.is_empty());
        assert_eq!(pruned.omitted_children, 1);

        let kept = render(
            &owner,
            &lines,
            RenderOptions {
                depth: 2,
                include_locals: true,
                ..RenderOptions::default()
            },
        );
        assert_eq!(kept.children.len(), 1);
        assert_eq!(kept.children[0].name_path.as_deref(), Some("cachedInfo"));
        assert_eq!(kept.omitted_children, 0);
    }

    #[test]
    fn a_member_is_returned_either_way_and_counts_as_nothing_omitted() {
        let mut owner = node("Config", 0, 20);
        let mut member = node("Config/MAX_LEVEL", 1, 1);
        member.kind = 13;
        member.member = true;
        owner.children.push(member);

        let lines = LineIndex::new("");
        for include_locals in [false, true] {
            let rendered = render(
                &owner,
                &lines,
                RenderOptions {
                    depth: 1,
                    include_locals,
                    ..RenderOptions::default()
                },
            );
            assert_eq!(rendered.children.len(), 1);
            assert_eq!(rendered.omitted_children, 0);
        }
    }

    #[test]
    fn depth_zero_reports_nothing_omitted() {
        let mut owner = node("PlayerUtils/Init", 10, 20);
        let mut local = node("PlayerUtils/Init/playerMaid", 12, 12);
        local.kind = 13;
        owner.children.push(local);

        let rendered = render(&owner, &LineIndex::new(""), RenderOptions::default());
        assert!(rendered.children.is_empty());
        assert_eq!(rendered.omitted_children, 0);
    }

    #[test]
    fn a_rule_is_a_separator_and_a_dashed_sentence_is_not() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("  ___  "));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("- a list item"));
        assert!(!is_horizontal_rule(""));
    }

    #[test]
    fn documentation_over_the_ceiling_is_cut_and_says_so() {
        let long = "é".repeat(MAX_DOCUMENTATION_CHARS + 10);
        let capped = cap_documentation(long);
        assert!(capped.ends_with("[documentation truncated]"));
        assert_eq!(
            capped.chars().filter(|c| *c == 'é').count(),
            MAX_DOCUMENTATION_CHARS
        );

        let short = "fits".to_string();
        assert_eq!(cap_documentation(short.clone()), short);
    }

    #[test]
    fn a_point_is_either_a_name_path_or_a_line() {
        assert!(matches!(
            SymbolPoint::parse(Some("PlayerService/update"), None, None).unwrap(),
            SymbolPoint::NamePath("PlayerService/update")
        ));

        assert!(matches!(
            SymbolPoint::parse(None, Some(12), None).unwrap(),
            SymbolPoint::LineColumn {
                line: 12,
                column: 1
            }
        ));
        assert!(matches!(
            SymbolPoint::parse(None, Some(12), Some(5)).unwrap(),
            SymbolPoint::LineColumn {
                line: 12,
                column: 5
            }
        ));

        assert!(matches!(
            SymbolPoint::parse(Some("  "), Some(3), None).unwrap(),
            SymbolPoint::LineColumn { line: 3, .. }
        ));

        assert!(SymbolPoint::parse(Some("update"), Some(3), None).is_err());
        assert!(SymbolPoint::parse(None, Some(0), None).is_err());
        assert!(SymbolPoint::parse(None, None, Some(4)).is_err());
        assert!(SymbolPoint::parse(None, None, None).is_err());
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
