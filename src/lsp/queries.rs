use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use super::client;
use super::name_path::NamePathPattern;
use super::protocol::{
    Diagnostic, Documentation, InlayHint, Location, Position, Range, Severity, SignatureHelp,
    inlay_hint_kind_label, is_low_level_kind,
};
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

const POINT_HINT: &str = "name the symbol with name_path, or give the line and column of a use of \
                          it; line and column are 1-based, as every Biskit result reports them";

/// Ceiling on the documentation half of a hover, which is prose and runs to hundreds of lines on a
/// well documented Roblox API member.
const MAX_DOCUMENTATION_CHARS: usize = 4_000;

/// luau-lsp answers `typeDefinition` from the *type name*, not from the value it annotates, so a
/// name path that resolves to a variable lands on a position the server has nothing to say about.
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

const RENAME_UNSUPPORTED_NOTE: &str = "this luau-lsp build does not implement textDocument/rename, \
                                       so no edit plan exists. The references below are every use \
                                       the server can see; they are not a rename plan, and a \
                                       same-named symbol elsewhere is not among them.";

const RENAME_EMPTY_NOTE: &str = "the language server produced no edits for this rename. The \
                                 references below are every use it can see; they are not a rename \
                                 plan.";

const RENAME_DECLINED_NOTE: &str = "the language server declined the rename. The references below \
                                    are every use it can see; they are not a rename plan.";

/// Words that cannot be used as a Luau identifier.
const LUAU_KEYWORDS: [&str; 21] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

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
    /// The returned expression as written, so a module that returns something unusual still says
    /// what it returns.
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

/// One replacement a rename would make. Positions are 1-based and the end is exclusive of the
/// character it names, matching how the language server described the range.
#[derive(Debug, Clone, Serialize)]
pub struct RenameEdit {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub old_text: String,
    pub new_text: String,
}

/// A rename that was planned and never applied.
#[derive(Debug, Clone, Serialize)]
pub struct RenamePlan {
    pub symbol: String,
    pub new_name: String,
    pub files: usize,
    pub total_edits: usize,
    pub edits: BTreeMap<String, Vec<RenameEdit>>,
    /// Why there is no plan, on the paths where the server could not produce one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The fallback answer that comes with `note`: every reference the server can see.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<ReferencesByFile>,
}

/// Where a request points inside a file.
///
/// A name path is what an agent holding a symbol has; a line and column is what an agent holding a
/// call site, an expression, or a diagnostic has. Both reach the same LSP position.
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

/// A position the server can be asked about, and what Biskit knows sits there.
struct ResolvedPoint {
    path: PathBuf,
    relative_path: String,
    position: Position,
    /// Absent when the position falls outside every symbol in the file.
    symbol: Option<SymbolNode>,
}

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

    /// Resolves either kind of pointer into one position in one file.
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

        // The containing symbol is context on the answer rather than part of it, so a file whose
        // symbol tree cannot be built still resolves to a position the server can be asked about.
        let symbols = session
            .document_symbols(&path)
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
            signature,
            documentation: include_documentation
                .then(|| cap_documentation(documentation))
                .filter(|text| !text.is_empty()),
        })
    }

    /// Where the *type* of a symbol is declared, which in Luau is usually an `export type` in some
    /// other module.
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
    ///
    /// This is the cheapest way to see what a function's arguments and returns actually resolve
    /// to: positions and short labels, with none of the body they were inferred from.
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

        // luau-lsp answers with the hints for the whole document whatever range it was asked for,
        // so a caller who asked about ten lines would otherwise be handed the file.
        let mut in_range: Vec<InlayHint> = hints
            .into_iter()
            .filter(|hint| (from..=to).contains(&hint.position.line))
            .collect();
        // Two hints on one line arrive in whichever order the server inferred them, which is not
        // the order they are read in.
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

    /// The public surface of a ModuleScript: what its returned value exposes, plus the types it
    /// exports, and none of the body either was implemented in.
    ///
    /// This is the question an agent opening an unfamiliar module actually has. Answering it by
    /// reading the file costs the whole file, and answering it with `get_symbols_overview` costs
    /// every local the module happens to declare alongside the handful it hands back.
    pub async fn module_api(&self, relative_path: &str, max_exports: usize) -> Result<ModuleApi> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let (symbols, content) = session.document_symbols(&path).await?;
        let relative = self.project().relativize(&path)?;

        // Comments are blanked rather than removed so a `return` inside one is not mistaken for
        // the module's own, and every line number still names the line it was written on.
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
            // A table literal's text is an opening brace and the rest of the file, which says
            // nothing the note does not say better.
            returns: (return_kind != "table_literal").then_some(expression),
            return_kind,
            exports,
            types,
            truncated,
            note,
        })
    }

    /// Every edit a rename would make, without making any of them.
    ///
    /// Biskit writes no source, so the plan is the answer: the agent applies it with its own edit
    /// tools and cannot miss a call site the way a grep-driven rename does.
    pub async fn plan_rename(
        &self,
        name_path: &str,
        relative_path: &str,
        new_name: &str,
        max_references: usize,
    ) -> Result<RenamePlan> {
        validate_identifier(new_name)?;

        let session = self.handle.session().await?;
        let (path, symbol, position) = self.locate_one(&session, name_path, relative_path).await?;

        let workspace_edit = match session.rename(&path, position, new_name).await {
            Ok(Some(edit)) => edit,
            Ok(None) => {
                return self
                    .rename_fallback(
                        &session,
                        &symbol,
                        new_name,
                        &path,
                        position,
                        max_references,
                        RENAME_EMPTY_NOTE.to_string(),
                    )
                    .await;
            }
            // A server that has gone away is not a decline: reporting references it can no longer
            // produce would dress a dead session up as an answer.
            Err(error) if client::is_unavailable(&error) => return Err(error),
            Err(error) => {
                let note = if client::is_unsupported(&error) {
                    RENAME_UNSUPPORTED_NOTE.to_string()
                } else {
                    match client::declined_reason(&error) {
                        Some(reason) => format!("{RENAME_DECLINED_NOTE} It said: {reason}"),
                        None => RENAME_DECLINED_NOTE.to_string(),
                    }
                };
                return self
                    .rename_fallback(
                        &session,
                        &symbol,
                        new_name,
                        &path,
                        position,
                        max_references,
                        note,
                    )
                    .await;
            }
        };

        let mut edits: BTreeMap<String, Vec<RenameEdit>> = BTreeMap::new();
        let mut total_edits = 0;

        for (uri, text_edits) in workspace_edit.into_edits_by_uri() {
            let Ok(target) = uri::to_path(&uri) else {
                continue;
            };
            let Ok(relative) = self.project().relativize(&target) else {
                continue;
            };
            let content = match session.ensure_open(&target).await {
                Ok(file) => file.content,
                Err(_) => Arc::from(""),
            };
            let lines = LineIndex::new(&content);

            let bucket = edits.entry(relative).or_default();
            for edit in text_edits {
                bucket.push(RenameEdit {
                    line: edit.range.start.line + 1,
                    column: edit.range.start.character + 1,
                    end_line: edit.range.end.line + 1,
                    end_column: edit.range.end.character + 1,
                    old_text: slice_range(&lines, edit.range),
                    new_text: edit.new_text,
                });
                total_edits += 1;
            }
        }

        if total_edits == 0 {
            return self
                .rename_fallback(
                    &session,
                    &symbol,
                    new_name,
                    &path,
                    position,
                    max_references,
                    RENAME_EMPTY_NOTE.to_string(),
                )
                .await;
        }

        // Applying edits from the bottom of a file upwards keeps earlier positions valid, which is
        // only possible if the caller is handed them in a known order.
        for bucket in edits.values_mut() {
            bucket.sort_by_key(|edit| (edit.line, edit.column));
        }

        Ok(RenamePlan {
            symbol: symbol.name_path,
            new_name: new_name.to_string(),
            files: edits.len(),
            total_edits,
            edits,
            note: None,
            references: None,
        })
    }

    /// What a rename plan degrades to when the server will not produce one.
    #[allow(clippy::too_many_arguments)]
    async fn rename_fallback(
        &self,
        session: &Session,
        symbol: &SymbolNode,
        new_name: &str,
        path: &Path,
        position: Position,
        max_references: usize,
        note: String,
    ) -> Result<RenamePlan> {
        let references = self
            .references_at(session, path, position, max_references, 0)
            .await
            .unwrap_or_default();

        Ok(RenamePlan {
            symbol: symbol.name_path.clone(),
            new_name: new_name.to_string(),
            files: 0,
            total_edits: 0,
            edits: BTreeMap::new(),
            note: Some(note),
            references: Some(references.references),
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
        self.references_at(&session, &path, position, max_results, context_lines)
            .await
    }

    /// `find_referencing_symbols` from a position that has already been resolved.
    async fn references_at(
        &self,
        session: &Session,
        path: &Path,
        position: Position,
        max_results: usize,
        context_lines: usize,
    ) -> Result<ReferenceSearchResult> {
        let locations = session.references(path, position, false).await?;

        // Collecting one past the cap is what makes a complete result set distinguishable
        // from a truncated one.
        let probe = max_results.saturating_add(1);
        let mut references: Vec<(String, ReferenceMatch)> = Vec::new();

        let wanted: Vec<Location> = locations
            .into_iter()
            .filter(|location| !is_declaration_site(location, path, position))
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

/// The last `return` written at the start of a line, which in Luau is the module's own.
///
/// A `return` inside a function body is indented; one at column zero closes the chunk. Taking the
/// last of them rather than the first means a module that returns early under a guard still
/// reports what it hands back in the ordinary case.
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

/// The name of the value a module returns, where the return statement names one.
fn returned_name(expression: &str) -> (Option<String>, Option<&'static str>) {
    let trimmed = expression.trim();
    if trimmed.starts_with("function") {
        return (None, Some("function"));
    }
    if trimmed.starts_with('{') {
        return (None, Some("table_literal"));
    }
    // `return setmetatable(Class, Class)` is how a Luau class module hands back its table.
    if let Some(rest) = trimmed.strip_prefix("setmetatable(") {
        let first = rest.split(',').next().unwrap_or_default().trim();
        return (identifier(first), None);
    }
    (identifier(trimmed), None)
}

/// The whole of `text`, when the whole of it is one identifier.
///
/// A partial match would be worse than none: `return Combat.new` names a function, and reporting
/// the members of `Combat` as the module's surface would be wrong rather than incomplete.
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

/// Lines an `export type` declaration is allowed to run to before it is cut.
const MAX_TYPE_DECLARATION_LINES: usize = 40;

/// Every `export type` in a file, quoted from the source rather than from the blanked copy.
///
/// The declarations are found in the blanked text so a type written inside a comment is not
/// reported, and the text is taken from the real source so the answer reads as it was written.
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

/// Whether a type declaration is obviously unfinished at the end of a line, which is how a union
/// written one variant per line reads.
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

/// Splits hover markdown into its code half and its prose half.
///
/// luau-lsp answers with the resolved type in a fenced block followed by whatever doc comment it
/// found, so the fences are what separate the two rather than a heading or a blank line. Hover with
/// no fence at all is taken as all signature: a bare type is what the server had to say about the
/// position, and filing it under documentation would hide it behind a flag.
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

/// The `---` luau-lsp puts between the type and the docs is a separator, not documentation.
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

/// Refuses a rename target that could not be a Luau name before the server is asked about it.
fn validate_identifier(name: &str) -> Result<()> {
    let mut characters = name.chars();
    let head_ok = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    let tail_ok = characters.all(|character| character.is_ascii_alphanumeric() || character == '_');

    if !head_ok || !tail_ok {
        bail_hint!(
            "a Luau name starts with a letter or an underscore and carries only letters, digits, \
             and underscores";
            "new_name is not a valid Luau identifier: {name:?}"
        );
    }
    if LUAU_KEYWORDS.contains(&name) {
        bail_hint!("pick a name that is not reserved"; "new_name is a Luau keyword: {name}");
    }
    Ok(())
}

/// The text a range covers, so a planned edit says what it would replace.
fn slice_range(lines: &LineIndex<'_>, range: Range) -> String {
    let start_line = range.start.line as usize;
    let end_line = range.end.line as usize;
    let text = lines.text(start_line, end_line);
    if text.is_empty() {
        return String::new();
    }

    if start_line == end_line {
        return text
            .chars()
            .skip(range.start.character as usize)
            .take(range.end.character.saturating_sub(range.start.character) as usize)
            .collect();
    }

    let mut spanned: Vec<String> = text.split('\n').map(str::to_string).collect();
    if let Some(first) = spanned.first_mut() {
        *first = first.chars().skip(range.start.character as usize).collect();
    }
    if let Some(last) = spanned.last_mut() {
        *last = last.chars().take(range.end.character as usize).collect();
    }
    spanned.join("\n")
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
        // A member of a table is part of what the table is, whatever kind the server gave it. The
        // low-level filter is aimed at locals declared inside a body, which are noise here.
        node.children
            .iter()
            .filter(|child| child.member || !is_low_level_kind(child.kind))
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

    /// The module's own return is the one at column zero. Every other `return` in a module belongs
    /// to a function inside it, and taking one of those would report the wrong surface entirely.
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

    /// `return Combat.new` hands back a function, not the table, so reporting the table's members
    /// as the module's surface would be a wrong answer rather than a missing one.
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

    /// The declarations are found in the blanked copy so that a type written inside a comment is
    /// not reported as one the module exports.
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
    fn a_rename_target_that_could_not_be_a_luau_name_is_refused() {
        assert!(validate_identifier("updateAll").is_ok());
        assert!(validate_identifier("_private2").is_ok());

        for refused in ["", "2fast", "has space", "has-dash", "PlayerService:update"] {
            assert!(
                validate_identifier(refused).is_err(),
                "accepted {refused:?}"
            );
        }
        assert!(
            validate_identifier("end").is_err(),
            "a keyword is not a name"
        );
    }

    #[test]
    fn a_planned_edit_reports_the_text_it_would_replace() {
        let content = "local PlayerService = {}\nfunction PlayerService:update()\nend\n";
        let lines = LineIndex::new(content);

        let single = Range {
            start: position(1, 23),
            end: position(1, 29),
        };
        assert_eq!(slice_range(&lines, single), "update");

        let spanning = Range {
            start: position(0, 6),
            end: position(1, 8),
        };
        assert_eq!(
            slice_range(&lines, spanning),
            "PlayerService = {}\nfunction"
        );
    }

    #[test]
    fn a_point_is_either_a_name_path_or_a_line() {
        assert!(matches!(
            SymbolPoint::parse(Some("PlayerService/update"), None, None).unwrap(),
            SymbolPoint::NamePath("PlayerService/update")
        ));

        // Columns are 1-based on the way in, and default to the start of the line.
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

        // An empty name path is not a pointer, so it falls through to the line.
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
