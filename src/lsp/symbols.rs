use serde::{Deserialize, Serialize};

use super::name_path::strip_overload_suffix;
use super::protocol::{DocumentSymbol, DocumentSymbolResponse, Position, Range, symbol_kind_label};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolNode {
    pub name: String,
    pub name_path: String,
    pub kind: u32,
    pub detail: Option<String>,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<SymbolNode>,
}

impl SymbolNode {
    pub fn kind_label(&self) -> &'static str {
        symbol_kind_label(self.kind)
    }

    pub fn ancestors(&self) -> Vec<String> {
        self.name_path.split('/').map(str::to_string).collect()
    }

    pub fn contains(&self, position: Position) -> bool {
        self.range.contains(position)
    }

    /// Position to aim LSP requests at. For a dotted or colon member the selection range starts
    /// on the containing table, which would resolve references to the table instead of the
    /// member, so seek forward to the leaf name.
    pub fn target_position(&self, content: &str) -> Position {
        let start = self.selection_range.start;
        let Some(line) = content.lines().nth(start.line as usize) else {
            return start;
        };

        let from = (start.character as usize).min(line.len());
        let window = &line[from..];
        let Some(offset) = find_identifier(window, strip_overload_suffix(&self.name)) else {
            return start;
        };
        Position {
            line: start.line,
            character: start.character + offset as u32,
        }
    }

    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a SymbolNode)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }

    /// Deepest symbol whose range covers `position`.
    pub fn innermost_at(nodes: &[SymbolNode], position: Position) -> Option<&SymbolNode> {
        let mut best: Option<&SymbolNode> = None;
        for node in nodes {
            if !node.contains(position) {
                continue;
            }
            best = Some(SymbolNode::innermost_at(&node.children, position).unwrap_or(node));
        }
        best
    }
}

/// Finds `needle` in `haystack` only where it stands alone as an identifier.
fn find_identifier(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let bytes = haystack.as_bytes();
    let boundary = |byte: u8| !(byte.is_ascii_alphanumeric() || byte == b'_');

    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let before_ok = start == 0 || boundary(bytes[start - 1]);
        let after_ok = end == bytes.len() || boundary(bytes[end]);
        if before_ok && after_ok {
            return Some(start);
        }
        from = start + 1;
    }
    None
}

pub fn build_tree(response: DocumentSymbolResponse) -> Vec<SymbolNode> {
    match response {
        DocumentSymbolResponse::Nested(symbols) => convert_siblings(&symbols, ""),
        DocumentSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let name_path = match symbol.container_name.filter(|name| !name.is_empty()) {
                    Some(container) => format!("{container}/{}", symbol.name),
                    None => symbol.name.clone(),
                };
                SymbolNode {
                    name: symbol.name,
                    name_path,
                    kind: symbol.kind,
                    detail: None,
                    range: symbol.location.range,
                    selection_range: symbol.location.range,
                    children: Vec::new(),
                }
            })
            .collect(),
    }
}

fn convert_siblings(symbols: &[DocumentSymbol], prefix: &str) -> Vec<SymbolNode> {
    let disambiguated = disambiguate(symbols);

    symbols
        .iter()
        .zip(disambiguated)
        .map(|(symbol, name)| {
            // luau-lsp reports members flat, dot-separated for fields ("PlayerService.addScore")
            // and colon-separated for methods ("PlayerService:addScore"), so both separators
            // become name path segments and the leaf becomes the display name.
            let segments: Vec<&str> = name
                .split(['.', ':'])
                .filter(|part| !part.is_empty())
                .collect();
            let leaf = segments
                .last()
                .copied()
                .unwrap_or(name.as_str())
                .to_string();
            let joined = if segments.is_empty() {
                name.clone()
            } else {
                segments.join("/")
            };
            let name_path = if prefix.is_empty() {
                joined
            } else {
                format!("{prefix}/{joined}")
            };

            SymbolNode {
                name: leaf,
                children: convert_siblings(&symbol.children, &name_path),
                name_path,
                kind: symbol.kind,
                detail: symbol.detail.clone(),
                range: symbol.range,
                selection_range: symbol.selection_range,
            }
        })
        .collect()
}

/// Siblings sharing a name get a `[n]` suffix so each name path stays addressable.
fn disambiguate(symbols: &[DocumentSymbol]) -> Vec<String> {
    let mut totals = std::collections::HashMap::<&str, usize>::new();
    for symbol in symbols {
        *totals.entry(symbol.name.as_str()).or_default() += 1;
    }

    let mut seen = std::collections::HashMap::<&str, usize>::new();
    symbols
        .iter()
        .map(|symbol| {
            if totals.get(symbol.name.as_str()).copied().unwrap_or(0) < 2 {
                return symbol.name.clone();
            }
            let index = seen.entry(symbol.name.as_str()).or_default();
            let rendered = format!("{}[{index}]", symbol.name);
            *index += 1;
            rendered
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start_line: u32, end_line: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: 0,
            },
        }
    }

    fn symbol(name: &str, children: Vec<DocumentSymbol>) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_string(),
            detail: None,
            kind: 12,
            range: range(0, 10),
            selection_range: range(0, 0),
            children,
        }
    }

    #[test]
    fn name_paths_chain_through_ancestors() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![symbol(
            "PlayerService",
            vec![symbol("update", vec![])],
        )]));
        assert_eq!(tree[0].name_path, "PlayerService");
        assert_eq!(tree[0].children[0].name_path, "PlayerService/update");
    }

    #[test]
    fn dotted_luau_names_become_name_path_segments() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![symbol(
            "PlayerService.addScore",
            vec![],
        )]));
        assert_eq!(tree[0].name_path, "PlayerService/addScore");
        assert_eq!(tree[0].name, "addScore");
        assert_eq!(
            tree[0].ancestors(),
            vec!["PlayerService".to_string(), "addScore".to_string()]
        );
    }

    #[test]
    fn colon_luau_method_names_become_name_path_segments() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![symbol(
            "PlayerUtils:GetPlayerMaid",
            vec![],
        )]));
        assert_eq!(tree[0].name_path, "PlayerUtils/GetPlayerMaid");
        assert_eq!(tree[0].name, "GetPlayerMaid");
        assert_eq!(
            tree[0].ancestors(),
            vec!["PlayerUtils".to_string(), "GetPlayerMaid".to_string()]
        );
    }

    #[test]
    fn target_position_seeks_past_the_owner_table() {
        let mut method = symbol("PlayerUtils:GetPlayerMaid", vec![]);
        method.selection_range = Range {
            start: Position {
                line: 0,
                character: 9,
            },
            end: Position {
                line: 0,
                character: 34,
            },
        };

        let tree = build_tree(DocumentSymbolResponse::Nested(vec![method]));
        let content = "function PlayerUtils:GetPlayerMaid(player)\nend\n";

        assert_eq!(
            tree[0].target_position(content),
            Position {
                line: 0,
                character: 21
            }
        );
    }

    #[test]
    fn target_position_ignores_the_disambiguation_suffix() {
        let mut first = symbol("PlayerUtils:Init", vec![]);
        first.selection_range = Range {
            start: Position {
                line: 0,
                character: 9,
            },
            end: Position {
                line: 0,
                character: 25,
            },
        };
        let second = first.clone();

        let tree = build_tree(DocumentSymbolResponse::Nested(vec![first, second]));
        assert_eq!(tree[0].name_path, "PlayerUtils/Init[0]");
        assert_eq!(tree[1].name_path, "PlayerUtils/Init[1]");

        let content = "function PlayerUtils:Init()\nend\n";
        assert_eq!(
            tree[0].target_position(content),
            Position {
                line: 0,
                character: 21
            }
        );
    }

    #[test]
    fn duplicate_siblings_are_indexed() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![
            symbol("overloaded", vec![]),
            symbol("overloaded", vec![]),
            symbol("unique", vec![]),
        ]));
        assert_eq!(tree[0].name_path, "overloaded[0]");
        assert_eq!(tree[1].name_path, "overloaded[1]");
        assert_eq!(tree[2].name_path, "unique");
    }

    #[test]
    fn innermost_at_picks_the_deepest_container() {
        let mut outer = symbol("Outer", vec![]);
        outer.range = range(0, 20);
        let mut inner = symbol("Inner", vec![]);
        inner.range = range(5, 10);
        outer.children.push(inner);

        let tree = build_tree(DocumentSymbolResponse::Nested(vec![outer]));
        let found = SymbolNode::innermost_at(
            &tree,
            Position {
                line: 7,
                character: 0,
            },
        )
        .unwrap();
        assert_eq!(found.name, "Inner");
    }
}
