use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use super::{SymbolQuery, check_line_range};
use crate::lines::LineIndex;
use crate::lsp::protocol::{Diagnostic, Severity};
use crate::lsp::session::ensure_luau_file;
use crate::lsp::symbols::SymbolNode;
use crate::lsp::uri;

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

impl<'a> SymbolQuery<'a> {
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
        let file = session.ensure_open(&path).await?;
        check_line_range(
            relative_path,
            &LineIndex::new(&file.content),
            start_line,
            end_line,
        )?;

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

        let file = session.ensure_open(&path).await?;
        let lines = LineIndex::new(&file.content);
        let mut grouped = self
            .file_diagnostics(
                relative_path,
                Some(lines.clamp_line(symbol.range.start.line as usize) as u32 + 1),
                Some(lines.clamp_line(symbol.range.end.line as usize) as u32 + 1),
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
        code @ 1..=4 => Ok(Severity::from_code(Some(code))),
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
    use super::super::tests::{node, range};
    use super::*;

    fn diagnostic(line: u32, message: &str) -> Diagnostic {
        Diagnostic {
            range: range(line),
            severity: Some(1),
            code: None,
            source: None,
            message: message.to_string(),
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
}
