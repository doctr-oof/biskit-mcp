mod diagnostics;
mod hover;
mod inlay;
mod module_api;
mod references;
mod render;
mod symbols;

pub use self::diagnostics::{
    DiagnosticEntry, GroupedDiagnostics, SeverityGroup, severity_from_input,
};
pub use self::hover::{SignatureEntry, SignatureHelpResult, SignatureParameter, SymbolExplanation};
pub use self::inlay::{InlayHintEntry, InlayHintResult};
pub use self::module_api::{ExportedType, ModuleApi, ModuleExport};
pub use self::references::{ReferenceMatch, ReferenceSearchResult, ReferencesByFile};
pub use self::render::RenderOptions;
pub use self::symbols::{
    FindSymbolRequest, SymbolMatch, SymbolOverviewResult, SymbolSearchResult, SymbolsByFile,
    check_symbol_kinds, prefilter_by_literal,
};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use self::hover::{split_hover, strip_unbound_generics};
use super::name_path::NamePathPattern;
use super::protocol::Position;
use super::session::{LanguageServerHandle, Session, ensure_luau_file};
use super::symbols::SymbolNode;
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::project::Project;

const NAME_PATH_HINT: &str = "a name path is a symbol name such as \"update\", optionally \
                              qualified with its owners as \"PlayerService:update\"; prefix \"/\" \
                              to anchor it to the top level of the file";

const POINT_HINT: &str = "name the symbol with name_path, or give the line and column of a use of \
                          it; line and column are 1-based, as every Biskit result reports them";

const COLUMN_HINT: &str = "columns are 1-based and count UTF-16 code units, as every Biskit result \
                           reports them; get_inlay_hints reports the columns of interest on a \
                           line, and omitting column aims at its start";

const LINE_HINT: &str = "line numbers are 1-based; get_symbols_overview shows where the file's \
                         symbols start and end";

const LINE_RANGE_HINT: &str = "start_line and end_line are 1-based and inclusive, as every Biskit \
                               result reports them; omit both to report on the whole file";

const MAX_DETAIL_HOVERS: usize = 200;

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
        check_line_bound("line", line, relative_path, &lines, LINE_HINT)?;

        let text = lines.slice(line as usize - 1, line as usize - 1);
        let width = crate::lines::byte_to_utf16_column(text, text.len());
        if column as usize - 1 > width {
            bail_hint!(
                COLUMN_HINT;
                "column {column} is past the end of {relative_path}:{line}, which is {width} \
                 columns wide"
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
}

/// A 1-based line number refused unless it names a line that `lines` actually holds.
///
/// `name` is the parameter the caller took the number from, so the message names what to correct.
fn check_line_bound(
    name: &str,
    line: u32,
    relative_path: &str,
    lines: &LineIndex<'_>,
    hint: &str,
) -> Result<()> {
    if line == 0 {
        return Err(crate::errors::hinted(
            format!("{name} is 1-based, so 0 names no line"),
            hint,
        ));
    }
    if line as usize > lines.len() {
        return Err(crate::errors::hinted(
            format!(
                "{name} {line} is past the end of {relative_path}, which has {} lines",
                lines.len()
            ),
            hint,
        ));
    }
    Ok(())
}

/// Both ends of an inclusive line range refused unless they cover at least one line of the file.
///
/// Nothing is clamped: a range that answers is a range that was in range.
fn check_line_range(
    relative_path: &str,
    lines: &LineIndex<'_>,
    start_line: Option<u32>,
    end_line: Option<u32>,
) -> Result<()> {
    for (name, value) in [("start_line", start_line), ("end_line", end_line)] {
        if let Some(line) = value {
            check_line_bound(name, line, relative_path, lines, LINE_RANGE_HINT)?;
        }
    }

    if let (Some(from), Some(to)) = (start_line, end_line)
        && from > to
    {
        return Err(crate::errors::hinted(
            format!("start_line {from} is past end_line {to}, so the range covers no lines"),
            LINE_RANGE_HINT,
        ));
    }
    Ok(())
}

fn group_by_file<T>(matches: Vec<(String, T)>) -> BTreeMap<String, Vec<T>> {
    let mut grouped: BTreeMap<String, Vec<T>> = BTreeMap::new();
    for (relative_path, item) in matches {
        grouped.entry(relative_path).or_default().push(item);
    }
    grouped
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::Range;

    pub(super) fn position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    pub(super) fn range(line: u32) -> Range {
        Range {
            start: position(line, 0),
            end: position(line, 10),
        }
    }

    pub(super) fn node(name_path: &str, start_line: u32, end_line: u32) -> SymbolNode {
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
    fn a_range_that_covers_no_lines_is_refused_rather_than_answered_empty() {
        let content = "one\ntwo\nthree\n";
        let lines = LineIndex::new(content);
        let check = |from, to| check_line_range("src/Alpha.luau", &lines, from, to);

        assert!(check(None, None).is_ok());
        assert!(check(Some(1), None).is_ok());
        assert!(check(Some(2), Some(2)).is_ok());
        assert!(check(Some(1), Some(3)).is_ok());

        let inverted = check(Some(3), Some(1)).unwrap_err();
        assert!(
            inverted.to_string().contains("past end_line"),
            "unexpected: {inverted}"
        );

        for (start, end) in [(Some(0), None), (None, Some(0))] {
            let error = check(start, end).unwrap_err();
            assert!(error.to_string().contains("1-based"), "unexpected: {error}");
        }
    }

    #[test]
    fn a_line_past_the_end_of_the_file_is_refused_with_the_real_length() {
        let content = "one\ntwo\nthree\n";
        let lines = LineIndex::new(content);

        for (from, to) in [(Some(4), Some(9)), (Some(1), Some(4)), (Some(9), None)] {
            let error =
                check_line_range("src/Alpha.luau", &lines, from, to).expect_err("{from:?}..{to:?}");
            assert!(
                error
                    .to_string()
                    .contains("is past the end of src/Alpha.luau, which has 3 lines"),
                "unexpected: {error}"
            );
        }

        let point = check_line_bound("line", 4, "src/Alpha.luau", &lines, LINE_HINT).unwrap_err();
        assert_eq!(
            point.to_string(),
            "line 4 is past the end of src/Alpha.luau, which has 3 lines"
        );
        assert!(check_line_bound("line", 3, "src/Alpha.luau", &lines, LINE_HINT).is_ok());
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
}
