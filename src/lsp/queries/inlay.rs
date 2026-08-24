use anyhow::Result;
use serde::Serialize;

use super::{SymbolQuery, check_line_range};
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::lsp::client;
use crate::lsp::protocol::{InlayHint, Position, Range, inlay_hint_kind_label};
use crate::lsp::session::ensure_luau_file;

const NO_HINTS_NOTE: &str = "no hints in this range: luau-lsp emits a hint only where a type or an \
                             argument name is not already written out. The luau-lsp.inlayHints.* \
                             keys under lsp.server_settings control which kinds are emitted.";

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
    /// Absent only on an empty file, which holds no line the range could name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub hints: Vec<InlayHintEntry>,
    #[serde(skip_serializing_if = "crate::serde_skip::is_false")]
    pub truncated: bool,
    /// Set only when the hint list is empty, where an empty list on its own reads as a failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'a> SymbolQuery<'a> {
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

        check_line_range(relative_path, &lines, start_line, end_line)?;

        if lines.is_empty() {
            return Ok(InlayHintResult {
                relative_path: relative,
                start_line: None,
                end_line: None,
                hints: Vec::new(),
                truncated: false,
                note: Some(format!("{relative_path} is empty")),
            });
        }

        let last = lines.len() as u32 - 1;
        let from = start_line.map_or(0, |line| line - 1);
        let to = end_line.map_or(last, |line| line - 1);
        let end_of_line = lines.slice(to as usize, to as usize);
        let range = Range {
            start: Position {
                line: from,
                character: 0,
            },
            end: Position {
                line: to,
                character: crate::lines::byte_to_utf16_column(end_of_line, end_of_line.len())
                    as u32,
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
            start_line: Some(from + 1),
            end_line: Some(to + 1),
            note: entries.is_empty().then(|| NO_HINTS_NOTE.to_string()),
            hints: entries,
            truncated,
        })
    }
}
