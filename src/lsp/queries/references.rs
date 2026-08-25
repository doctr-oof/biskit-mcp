use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use super::render::{RenderOptions, group_locations_by_file, render, snippet_around};
use super::{
    MAX_DETAIL_HOVERS, SymbolMatch, SymbolPoint, SymbolQuery, SymbolsByFile, attach_details,
    group_by_file,
};
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::lsp::client;
use crate::lsp::name_path::strip_overload_suffix;
use crate::lsp::protocol::{Location, Position, Range};
use crate::lsp::session::Session;
use crate::lsp::symbols::{SymbolNode, is_identifier_byte};
use crate::lsp::uri;

const DECLARATION_CONTEXT_LINES: usize = 1;

const TYPE_DEFINITION_HINT: &str = "aim line and column at the type's own name: in \
                                    `local config: PlayerConfig`, at `PlayerConfig` not `config`. \
                                    A value with no written annotation has no type declaration to \
                                    find; explain_symbol reports what it resolved to.";

const SELF_REFERENCE_NOTE: &str = "references marked resolved_by \"text\" were found by scanning \
                                   the declaring file for `self:` and `self.` uses. luau-lsp types \
                                   the implicit self of a colon-declared method as a fresh generic \
                                   rather than as the owner, so it reports no reference for those \
                                   call sites. They are matched on the symbol's own name, so \
                                   confirm the receiver before treating one as a call site.";

const TEXT_RESOLUTION: &str = "text";

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
    #[serde(skip_serializing_if = "crate::serde_skip::is_false")]
    pub truncated: bool,
    /// Set only when the answer carries a reference the text scan recovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'a> SymbolQuery<'a> {
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
            if let Some(symbol) = resolved.symbol.as_ref() {
                let content = session.ensure_open(&resolved.path).await?.content;
                if declares_a_type(&content, symbol) {
                    let options = RenderOptions {
                        depth: 0,
                        include_body,
                        include_detail,
                        include_locals: false,
                    };
                    let mut rendered = vec![render(symbol, &LineIndex::new(&content), options)];
                    if include_detail {
                        let mut budget = MAX_DETAIL_HOVERS;
                        attach_details(&session, &resolved.path, &mut rendered, &mut budget).await;
                    }
                    return Ok(SymbolsByFile::from([(resolved.relative_path, rendered)]));
                }
            }

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

    pub(super) async fn render_locations(
        &self,
        session: &Session,
        locations: Vec<Location>,
        include_body: bool,
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let mut rendered = SymbolsByFile::new();
        let mut budget = MAX_DETAIL_HOVERS;
        let mut outside: Vec<String> = Vec::new();
        for (target, group) in group_locations_by_file(locations) {
            let Ok(relative) = self.project().relativize(&target) else {
                outside.push(target.display().to_string());
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
                let declaration_end = node
                    .filter(|found| found.range.start.line == location.range.start.line)
                    .map(|found| found.range.end.line);
                here.push(SymbolMatch {
                    name_path: node.map(|found| found.name_path.clone()),
                    kind: node
                        .map(|found| found.kind_label().to_string())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    start_line: location.range.start.line + 1,
                    end_line: declaration_end.unwrap_or(location.range.end.line) + 1,
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

        if rendered.is_empty() && !outside.is_empty() {
            bail_hint!(
                "it is declared in a file Biskit cannot report on, so there is nothing to point \
                 at inside the project";
                "the language server resolved this outside the project root: {}",
                outside.join(", ")
            );
        }
        Ok(rendered)
    }
}

/// Whether `symbol` is the `type` or `export type` declaration on its own line.
///
/// A name path that already names a type declaration is the answer, and asking the language server
/// for a type definition at a declaration site is asking it what it points at, which is nothing.
fn declares_a_type(content: &str, symbol: &SymbolNode) -> bool {
    let lines = LineIndex::new(content);
    let line = lines.slice(
        symbol.range.start.line as usize,
        symbol.range.start.line as usize,
    );

    let rest = line
        .trim_start()
        .strip_prefix("export")
        .map_or(line.trim_start(), str::trim_start);
    let Some(rest) = rest.strip_prefix("type") else {
        return false;
    };
    rest.starts_with(|first: char| first.is_whitespace())
        && rest.trim_start().starts_with(&symbol.name)
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

fn is_declaration_site(location: &Location, path: &Path, position: Position) -> bool {
    location.range.start == position
        && uri::to_path(&location.uri).is_ok_and(|target| target == path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::tests::position;
    use super::*;

    #[test]
    fn a_name_path_that_lands_on_a_type_declaration_is_recognised_as_one() {
        let source = "local config = 1\n\
                      export type Amount = number | string\n\
                      type Id = string\n\
                      local Amounted = {}\n";

        let mut declaration = super::super::tests::node("Amount", 1, 1);
        declaration.name = "Amount".to_string();
        assert!(declares_a_type(source, &declaration));

        let mut unexported = super::super::tests::node("Id", 2, 2);
        unexported.name = "Id".to_string();
        assert!(declares_a_type(source, &unexported));

        let mut value = super::super::tests::node("config", 0, 0);
        value.name = "config".to_string();
        assert!(!declares_a_type(source, &value));

        let mut lookalike = super::super::tests::node("Amounted", 3, 3);
        lookalike.name = "Amounted".to_string();
        assert!(!declares_a_type(source, &lookalike));
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
