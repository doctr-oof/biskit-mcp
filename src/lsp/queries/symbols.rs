use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use super::render::{RenderOptions, render};
use super::{
    MAX_DETAIL_HOVERS, NAME_PATH_HINT, SymbolPoint, SymbolQuery, attach_details, group_by_file,
};
use crate::bail_hint;
use crate::lines::LineIndex;
use crate::lsp::client;
use crate::lsp::name_path::NamePathPattern;
use crate::lsp::protocol::Position;
use crate::lsp::session::ensure_luau_file;
use crate::lsp::symbols::SymbolNode;

const SCAN_ABORTED: &str = "the project scan stopped early because the language server stopped \
                            answering; restart it with restart_language_server";

const DETAIL_CAPPED_NOTE: &str = "detail was filled for the first symbols only: one hover request \
                                  per symbol is spent resolving a signature, and this answer hit \
                                  the ceiling. Narrow the answer with relative_path or a lower \
                                  depth, or ask explain_symbol about the symbols still missing a \
                                  detail.";

const DECLARATION_HINT: &str = "aim line and column at the symbol's own name at a use of it; \
                                name_path only resolves against symbols the named file declares, \
                                so it cannot start from a call site";

/// The highest `SymbolKind` the LSP specification defines.
const MAX_SYMBOL_KIND: u32 = 26;

const SYMBOL_KIND_HINT: &str = "the kinds are 1 File, 2 Module, 3 Namespace, 4 Package, 5 Class, \
                                6 Method, 7 Property, 8 Field, 9 Constructor, 10 Enum, \
                                11 Interface, 12 Function, 13 Variable, 14 Constant, 15 String, \
                                16 Number, 17 Boolean, 18 Array, 19 Object, 20 Key, 21 Null, \
                                22 EnumMember, 23 Struct, 24 Event, 25 Operator, \
                                26 TypeParameter";

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
    #[serde(skip_serializing_if = "crate::serde_skip::is_zero")]
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
    #[serde(skip_serializing_if = "crate::serde_skip::is_false")]
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

    /// Where a symbol is declared, addressed either by name or by the position of a use of it.
    ///
    /// A call site is not a document symbol, so a name path only ever resolves inside the file
    /// that declares the symbol. Line and column are how a caller asks about a symbol declared
    /// somewhere else.
    pub async fn find_declaration(
        &self,
        point: SymbolPoint<'_>,
        relative_path: &str,
        include_body: bool,
        include_detail: bool,
    ) -> Result<SymbolsByFile> {
        let session = self.handle.session().await?;
        let resolved = self.locate_point(&session, point, relative_path).await?;
        let locations = session
            .definition(&resolved.path, resolved.position)
            .await?;

        if locations.is_empty() {
            let Some(symbol) = resolved.symbol else {
                bail_hint!(
                    DECLARATION_HINT;
                    "the language server knows no declaration at {relative_path}:{}:{}",
                    resolved.position.line + 1,
                    resolved.position.character + 1
                );
            };
            let content = session.ensure_open(&resolved.path).await?.content;
            let options = RenderOptions {
                depth: 0,
                include_body,
                include_detail,
                include_locals: false,
            };
            let mut rendered = vec![render(&symbol, &LineIndex::new(&content), options)];
            if include_detail {
                let mut budget = MAX_DETAIL_HOVERS;
                attach_details(&session, &resolved.path, &mut rendered, &mut budget).await;
            }
            return Ok(SymbolsByFile::from([(resolved.relative_path, rendered)]));
        }

        self.render_locations(&session, locations, include_body, include_detail)
            .await
    }
}

/// Refuses a kind filter the LSP specification has no number for.
///
/// An out-of-range kind matches nothing, so without this the answer reads exactly like "the
/// project defines no such symbol", which is the one thing it does not mean.
pub fn check_symbol_kinds(field: &str, kinds: &[u32]) -> Result<()> {
    let Some(invalid) = kinds
        .iter()
        .copied()
        .find(|kind| !(1..=MAX_SYMBOL_KIND).contains(kind))
    else {
        return Ok(());
    };

    Err(crate::errors::hinted(
        format!("{field} takes LSP SymbolKind numbers from 1 to {MAX_SYMBOL_KIND}, got {invalid}"),
        SYMBOL_KIND_HINT,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_the_specification_has_no_number_for_is_refused() {
        assert!(check_symbol_kinds("include_kinds", &[]).is_ok());
        assert!(check_symbol_kinds("include_kinds", &[1, 12, 26]).is_ok());

        for (field, kinds) in [
            ("include_kinds", vec![9999]),
            ("exclude_kinds", vec![12, 0]),
            ("include_kinds", vec![27]),
        ] {
            let error = check_symbol_kinds(field, &kinds).unwrap_err();
            assert!(
                error.to_string().contains(field),
                "unexpected: {error}, for {kinds:?}"
            );
            assert!(
                error
                    .downcast_ref::<crate::errors::HintedError>()
                    .expect("the refusal carries the mapping")
                    .hint()
                    .contains("12 Function"),
                "the hint must spell the mapping out, since nothing else does"
            );
        }
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
}
