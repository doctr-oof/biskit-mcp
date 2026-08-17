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
    /// Set on a symbol nested under the owner its own name named, rather than under a symbol the
    /// language server itself reported it inside. A member is worth showing whatever its kind,
    /// where a local declared inside a function body is not.
    #[serde(default)]
    pub member: bool,
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

    /// Tightest symbol whose range covers `position`.
    ///
    /// A member nested under its owner is declared elsewhere in the file than the owner is, so the
    /// search cannot stop descending at a parent whose own range misses the position; the tightest
    /// range wins instead of the deepest nesting.
    pub fn innermost_at(nodes: &[SymbolNode], position: Position) -> Option<&SymbolNode> {
        let mut best: Option<&SymbolNode> = None;
        for node in nodes {
            let candidate = match SymbolNode::innermost_at(&node.children, position) {
                Some(nested) => nested,
                None if node.contains(position) => node,
                None => continue,
            };
            if best.is_none_or(|current| span(current.range) >= span(candidate.range)) {
                best = Some(candidate);
            }
        }
        best
    }
}

/// How much ground a range covers, for choosing between two symbols that both cover a position.
fn span(range: Range) -> (u32, u32) {
    let lines = range.end.line.saturating_sub(range.start.line);
    if lines == 0 {
        return (0, range.end.character.saturating_sub(range.start.character));
    }
    (lines, range.end.character)
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
        DocumentSymbolResponse::Flat(symbols) => nest_members(
            symbols
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
                        member: false,
                    }
                })
                .collect(),
        ),
    }
}

/// Nests each symbol under the owner its own name names.
///
/// luau-lsp reports table members as flat siblings carrying their owner in the name
/// ("Config.load"), so the hierarchy the name paths spell out is not the hierarchy the response
/// has. Rebuilding it is what gives `depth` something to descend into, and every name path is
/// preserved exactly, so a symbol is addressed the same way before and after.
///
/// A member whose owner is not itself reported stays where it is: dropping it under a parent that
/// does not exist would hide it from the top level with nothing to find it under.
fn nest_members(nodes: Vec<SymbolNode>) -> Vec<SymbolNode> {
    if nodes.len() < 2 || !nodes.iter().any(|node| node.name_path.contains('/')) {
        return nodes;
    }

    let mut parents = vec![None; nodes.len()];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    {
        let mut owners = std::collections::HashMap::<&str, usize>::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            owners.entry(node.name_path.as_str()).or_insert(index);
        }
        for (index, node) in nodes.iter().enumerate() {
            // Every owner has a strictly shorter name path than the symbol it owns, so the links
            // can never close a cycle.
            if let Some(parent) = owner_index(&owners, &node.name_path, index) {
                parents[index] = Some(parent);
                children[parent].push(index);
            }
        }
    }

    let roots: Vec<usize> = (0..nodes.len())
        .filter(|index| parents[*index].is_none())
        .collect();
    let mut slots: Vec<Option<SymbolNode>> = nodes.into_iter().map(Some).collect();
    roots
        .into_iter()
        .filter_map(|index| take_subtree(index, &children, &mut slots))
        .collect()
}

/// Nearest reported ancestor of `name_path`, longest match first, so `A/B/C` lands under `A/B`
/// where that exists and under `A` where it does not.
fn owner_index(
    owners: &std::collections::HashMap<&str, usize>,
    name_path: &str,
    index: usize,
) -> Option<usize> {
    let mut candidate = name_path;
    while let Some((prefix, _)) = candidate.rsplit_once('/') {
        match owners.get(prefix) {
            Some(&owner) if owner != index => return Some(owner),
            _ => candidate = prefix,
        }
    }
    None
}

fn take_subtree(
    index: usize,
    children: &[Vec<usize>],
    slots: &mut Vec<Option<SymbolNode>>,
) -> Option<SymbolNode> {
    let mut node = slots[index].take()?;
    for &child in &children[index] {
        if let Some(mut nested) = take_subtree(child, children, slots) {
            nested.member = true;
            node.children.push(nested);
        }
    }
    Some(node)
}

fn convert_siblings(symbols: &[DocumentSymbol], prefix: &str) -> Vec<SymbolNode> {
    let disambiguated = disambiguate(symbols);

    let converted = symbols
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
                member: false,
            }
        })
        .collect();

    nest_members(converted)
}

/// Siblings sharing a name get a `[n]` suffix so each name path stays addressable.
///
/// Sibling groups with no duplicate at all are the overwhelming majority, and this runs for every
/// group at every level of every scanned file, so the duplicate-free case avoids the hash maps
/// entirely and hands back the names as they stand.
pub fn disambiguate(symbols: &[DocumentSymbol]) -> Vec<String> {
    if !has_duplicate_names(symbols) {
        return symbols.iter().map(|symbol| symbol.name.clone()).collect();
    }

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

/// Sibling groups are short, so the quadratic scan beats building a set for the sizes that occur
/// in practice; the set is only worth its allocation once a group is large.
fn has_duplicate_names(symbols: &[DocumentSymbol]) -> bool {
    const LINEAR_SCAN_LIMIT: usize = 16;

    if symbols.len() < 2 {
        return false;
    }
    if symbols.len() <= LINEAR_SCAN_LIMIT {
        return symbols.iter().enumerate().any(|(index, symbol)| {
            symbols[index + 1..]
                .iter()
                .any(|other| other.name == symbol.name)
        });
    }

    let mut seen = std::collections::HashSet::with_capacity(symbols.len());
    symbols
        .iter()
        .any(|symbol| !seen.insert(symbol.name.as_str()))
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

    /// The fast path skips the counting maps, so both sides of its size threshold need covering.
    #[test]
    fn duplicates_are_found_in_groups_of_every_size() {
        for size in [2usize, 8, 17, 64] {
            let unique: Vec<DocumentSymbol> = (0..size)
                .map(|index| symbol(&format!("unique{index}"), vec![]))
                .collect();
            assert!(
                !has_duplicate_names(&unique),
                "group of {size} unique names reported a duplicate"
            );

            let mut duplicated = unique.clone();
            duplicated.push(symbol("unique0", vec![]));
            assert!(
                has_duplicate_names(&duplicated),
                "group of {size} plus a repeat missed the duplicate"
            );

            let tree = build_tree(DocumentSymbolResponse::Nested(duplicated));
            assert_eq!(tree[0].name_path, "unique0[0]");
            assert_eq!(tree[size].name_path, "unique0[1]");
        }
    }

    #[test]
    fn a_lone_sibling_is_never_indexed() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![symbol("only", vec![])]));
        assert_eq!(tree[0].name_path, "only");
    }

    #[test]
    fn flat_luau_members_nest_under_their_owner() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![
            symbol("RebirthConfig", vec![]),
            symbol("RebirthConfig.MAX_LEVEL", vec![]),
            symbol("RebirthConfig:Apply", vec![]),
        ]));

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name_path, "RebirthConfig");
        let names: Vec<&str> = tree[0]
            .children
            .iter()
            .map(|child| child.name_path.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["RebirthConfig/MAX_LEVEL", "RebirthConfig/Apply"]
        );
        assert!(tree[0].children.iter().all(|child| child.member));
        assert!(!tree[0].member);
    }

    #[test]
    fn a_member_nests_under_the_deepest_reported_owner() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![
            symbol("Config", vec![]),
            symbol("Config.Limits", vec![]),
            symbol("Config.Limits.Max", vec![]),
            symbol("Config.Other.Deep", vec![]),
        ]));

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].name_path, "Config/Limits");
        assert_eq!(
            tree[0].children[0].children[0].name_path,
            "Config/Limits/Max"
        );
        assert_eq!(tree[0].children[1].name_path, "Config/Other/Deep");
    }

    #[test]
    fn a_member_whose_owner_is_unreported_stays_top_level() {
        let tree = build_tree(DocumentSymbolResponse::Nested(vec![
            symbol("Elsewhere.helper", vec![]),
            symbol("main", vec![]),
        ]));

        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].name_path, "Elsewhere/helper");
        assert_eq!(tree[1].name_path, "main");
    }

    #[test]
    fn innermost_at_reaches_a_member_outside_its_owner_range() {
        let mut owner = symbol("Config", vec![]);
        owner.range = range(0, 0);
        let mut member = symbol("Config.load", vec![]);
        member.range = range(10, 20);

        let tree = build_tree(DocumentSymbolResponse::Nested(vec![owner, member]));
        let found = SymbolNode::innermost_at(
            &tree,
            Position {
                line: 15,
                character: 0,
            },
        )
        .unwrap();
        assert_eq!(found.name_path, "Config/load");
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
