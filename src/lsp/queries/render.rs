use std::path::PathBuf;

use super::SymbolMatch;
use crate::lines::LineIndex;
use crate::lsp::protocol::{Location, is_low_level_kind};
use crate::lsp::symbols::SymbolNode;
use crate::lsp::uri;

/// What a rendered symbol carries beyond its name, kind, and line range.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    pub depth: u32,
    pub include_body: bool,
    pub include_detail: bool,
    /// Descend into locals declared inside a body, which are pruned by default as noise.
    pub include_locals: bool,
}

pub(super) fn group_locations_by_file(locations: Vec<Location>) -> Vec<(PathBuf, Vec<Location>)> {
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

pub(super) fn render(
    node: &SymbolNode,
    lines: &LineIndex<'_>,
    options: RenderOptions,
) -> SymbolMatch {
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

fn extract_body(lines: &LineIndex<'_>, node: &SymbolNode) -> String {
    lines
        .text(node.range.start.line as usize, node.range.end.line as usize)
        .into_owned()
}

pub(super) fn snippet_around(lines: &LineIndex<'_>, line: u32, context: usize) -> String {
    let index = lines.clamp_line(line as usize);
    lines
        .text(index.saturating_sub(context), index + context)
        .into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::tests::{node, position, range};
    use super::*;
    use crate::lsp::protocol::Range;

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
}
