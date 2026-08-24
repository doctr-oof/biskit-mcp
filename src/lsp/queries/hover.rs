use anyhow::Result;
use serde::Serialize;

use super::render::group_locations_by_file;
use super::{POINT_HINT, ResolvedPoint, SymbolPoint, SymbolQuery};
use crate::bail_hint;
use crate::lsp::client;
use crate::lsp::protocol::{Documentation, Location, SignatureHelp};
use crate::lsp::session::Session;
use crate::lsp::symbols::{SymbolNode, find_identifier, is_identifier_byte};

const MAX_DOCUMENTATION_CHARS: usize = 4_000;

const OUTSIDE_ROOT_NOTE: &str = "the declaration resolved outside the project root, so no \
                                 name_path is reported";

const NO_SIGNATURES_NOTE: &str = "the language server answered with no signatures, and it does not \
                                  say why. The usual cause is a position outside the parentheses \
                                  of a call, so check that line and column name an argument \
                                  position rather than the function's declaration. luau-lsp also \
                                  answers with nothing at some positions that are inside a call, \
                                  among them the receiver of a `self:` method call, so a position \
                                  that looks right may still come back empty.";

/// What the language server resolved a symbol to, in the form an agent reads.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolExplanation {
    pub relative_path: String,
    pub line: u32,
    pub column: u32,
    /// The symbol the position names, not the one it sits inside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_path: Option<String>,
    /// Set when the declaration is in a file other than the one asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The symbol the position sits inside, which is a different question from `name_path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containing_symbol: Option<String>,
    /// The resolved type, taken from the code half of the hover.
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where the declaration behind a position landed.
enum DeclarationSite {
    Named {
        name_path: String,
        kind: String,
        relative_path: String,
    },
    OutsideRoot,
    Unknown,
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

impl<'a> SymbolQuery<'a> {
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

        let here = resolved
            .symbol
            .as_ref()
            .map(|symbol| (symbol.name_path.clone(), symbol.kind_label().to_string()));

        let (name_path, declared_in, kind, containing_symbol, note) = match point {
            SymbolPoint::NamePath(_) => {
                let (name_path, kind) = here.unzip();
                (name_path, None, kind, None, None)
            }
            SymbolPoint::LineColumn { .. } => {
                let containing = here.map(|(name_path, _)| name_path);
                match self.declaration_at(&session, &resolved).await {
                    DeclarationSite::Named {
                        name_path,
                        kind,
                        relative_path,
                    } => (
                        Some(name_path),
                        (relative_path != resolved.relative_path).then_some(relative_path),
                        Some(kind),
                        containing,
                        None,
                    ),
                    DeclarationSite::OutsideRoot => (
                        None,
                        None,
                        None,
                        containing,
                        Some(OUTSIDE_ROOT_NOTE.to_string()),
                    ),
                    DeclarationSite::Unknown => (None, None, None, containing, None),
                }
            }
        };

        Ok(SymbolExplanation {
            signature: strip_unbound_generics(&signature),
            relative_path: resolved.relative_path,
            line: resolved.position.line + 1,
            column: resolved.position.character + 1,
            name_path,
            declared_in,
            kind,
            containing_symbol,
            documentation: include_documentation
                .then(|| cap_documentation(documentation))
                .filter(|text| !text.is_empty()),
            note,
        })
    }

    /// The symbol a position names, resolved through the same lookup `find_declaration` makes.
    ///
    /// The innermost symbol at the position answers a different question — what the position sits
    /// inside — so a call site would come back named after its caller.
    async fn declaration_at(&self, session: &Session, resolved: &ResolvedPoint) -> DeclarationSite {
        let Ok(locations) = session.definition(&resolved.path, resolved.position).await else {
            return DeclarationSite::Unknown;
        };
        if locations.is_empty() {
            return DeclarationSite::Unknown;
        }

        let mut outside = false;
        for (target, group) in group_locations_by_file(locations) {
            let Ok(relative_path) = self.project().relativize(&target) else {
                outside = true;
                continue;
            };
            let Ok((symbols, _)) = self.handle.document_symbols(session, &target).await else {
                continue;
            };
            let Some(node) = declared_symbol(&symbols, &group) else {
                continue;
            };
            return DeclarationSite::Named {
                name_path: node.name_path.clone(),
                kind: node.kind_label().to_string(),
                relative_path,
            };
        }

        match outside {
            true => DeclarationSite::OutsideRoot,
            false => DeclarationSite::Unknown,
        }
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
                label: strip_unbound_generics(&signature.label),
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
}

/// The symbol one of `locations` declares, and never a symbol it merely sits inside.
///
/// A declaration the symbol tree does not carry, such as a local in some builds, resolves to its
/// enclosing function instead, which is the answer this whole path exists to avoid reporting.
fn declared_symbol<'a>(
    symbols: &'a [SymbolNode],
    locations: &[Location],
) -> Option<&'a SymbolNode> {
    locations.iter().find_map(|location| {
        let node = SymbolNode::innermost_at(symbols, location.range.start)?;
        let declares = node.selection_range.contains(location.range.start)
            || node.range.start == location.range.start;
        declares.then_some(node)
    })
}

pub(super) fn split_hover(markdown: &str) -> (String, String) {
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

pub(super) fn strip_unbound_generics(signature: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::{Position, Range};

    fn at(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn span(start: Position, end: Position) -> Range {
        Range { start, end }
    }

    fn node(name_path: &str, range: Range, selection_range: Range) -> SymbolNode {
        SymbolNode {
            name: name_path
                .rsplit('/')
                .next()
                .unwrap_or(name_path)
                .to_string(),
            name_path: name_path.to_string(),
            kind: 12,
            detail: None,
            range,
            selection_range,
            children: Vec::new(),
            member: false,
        }
    }

    fn located(range: Range) -> Location {
        Location {
            uri: "file:///project/src/EconomyService.luau".to_string(),
            range,
        }
    }

    fn economy_service() -> Vec<SymbolNode> {
        let mut owner = node(
            "EconomyService",
            span(at(0, 0), at(300, 0)),
            span(at(0, 6), at(0, 20)),
        );
        owner.children.push(node(
            "EconomyService/TakeMoney",
            span(at(219, 0), at(240, 3)),
            span(at(219, 25), at(219, 34)),
        ));
        vec![owner]
    }

    #[test]
    fn a_definition_on_a_symbols_own_name_names_that_symbol() {
        let symbols = economy_service();
        let found = declared_symbol(&symbols, &[located(span(at(219, 25), at(219, 34)))]).unwrap();
        assert_eq!(found.name_path, "EconomyService/TakeMoney");
    }

    #[test]
    fn a_definition_the_symbol_tree_does_not_carry_names_nothing() {
        let symbols = economy_service();
        assert!(
            declared_symbol(&symbols, &[located(span(at(225, 8), at(225, 12)))]).is_none(),
            "a position inside a body declares the enclosing function, not the local asked about"
        );
    }

    #[test]
    fn a_definition_at_the_start_of_a_declaration_is_taken() {
        let symbols = economy_service();
        let found = declared_symbol(&symbols, &[located(span(at(219, 0), at(219, 8)))]).unwrap();
        assert_eq!(found.name_path, "EconomyService/TakeMoney");
    }

    #[test]
    fn the_first_location_that_declares_something_answers() {
        let symbols = economy_service();
        let found = declared_symbol(
            &symbols,
            &[
                located(span(at(225, 8), at(225, 12))),
                located(span(at(219, 25), at(219, 34))),
            ],
        )
        .unwrap();
        assert_eq!(found.name_path, "EconomyService/TakeMoney");

        assert!(declared_symbol(&symbols, &[]).is_none());
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
}
