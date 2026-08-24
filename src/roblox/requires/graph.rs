use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use super::super::sourcemap::Sourcemap;
use super::Direction;
use super::build::GraphStamp;

/// A require Biskit could not resolve to a file, and why.
#[derive(Debug, Clone, Serialize)]
pub struct UnresolvedRequire {
    pub line: u32,
    /// The require as written, so the caller can go and look at it.
    pub expression: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub target: usize,
    pub line: u32,
    pub expression: String,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub relative_path: String,
    pub instance_path: Option<String>,
    pub dependencies: Vec<Edge>,
    pub dependents: Vec<usize>,
    pub unresolved: Vec<UnresolvedRequire>,
}

/// A module reached by walking the graph, and how it was reached.
#[derive(Debug, Clone, Serialize)]
pub struct ReachedModule {
    pub relative_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_path: Option<String>,
    /// Hops from the module the walk started at.
    pub depth: u32,
    /// The require that reached it, on the first hop that did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Which file the reaching require was written in, for hops past the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug)]
pub struct RequireGraph {
    pub(super) modules: Vec<Module>,
    pub(super) index: HashMap<String, usize>,
    pub(super) stamp: GraphStamp,
}

impl RequireGraph {
    pub fn stamp(&self) -> &GraphStamp {
        &self.stamp
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn module(&self, index: usize) -> &Module {
        &self.modules[index]
    }

    pub fn find(&self, relative_path: &str) -> Option<usize> {
        self.index
            .get(&super::super::sourcemap::normalize(relative_path))
            .copied()
    }

    /// Every module within `depth` hops of `start`, breadth first so the shallowest route to each one is the route reported.
    pub fn walk(
        &self,
        start: usize,
        direction: Direction,
        depth: u32,
        limit: usize,
    ) -> (Vec<ReachedModule>, bool) {
        let mut seen = vec![false; self.modules.len()];
        seen[start] = true;

        let mut reached = Vec::new();
        let mut frontier = vec![start];
        let mut truncated = false;

        for hop in 1..=depth.max(1) {
            let mut next = Vec::new();
            for &current in &frontier {
                for (target, line, expression) in self.neighbours(current, direction) {
                    if seen[target] {
                        continue;
                    }
                    seen[target] = true;
                    if reached.len() >= limit {
                        truncated = true;
                        continue;
                    }
                    let module = &self.modules[target];
                    reached.push(ReachedModule {
                        relative_path: module.relative_path.clone(),
                        instance_path: module.instance_path.clone(),
                        depth: hop,
                        via: expression,
                        from: (hop > 1).then(|| self.modules[current].relative_path.clone()),
                        line,
                    });
                    next.push(target);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        (reached, truncated)
    }

    fn neighbours(
        &self,
        index: usize,
        direction: Direction,
    ) -> Vec<(usize, Option<u32>, Option<String>)> {
        match direction {
            Direction::Dependencies => self.modules[index]
                .dependencies
                .iter()
                .map(|edge| (edge.target, Some(edge.line), Some(edge.expression.clone())))
                .collect(),
            Direction::Dependents => self.modules[index]
                .dependents
                .iter()
                .map(|&owner| {
                    let edge = self.modules[owner]
                        .dependencies
                        .iter()
                        .find(|edge| edge.target == index);
                    (
                        owner,
                        edge.map(|edge| edge.line),
                        edge.map(|edge| edge.expression.clone()),
                    )
                })
                .collect(),
        }
    }

    /// Require cycles, each reported as the loop it closes.
    pub fn cycles(&self, limit: usize) -> (Vec<Vec<String>>, bool) {
        const WHITE: u8 = 0;
        const GREY: u8 = 1;
        const BLACK: u8 = 2;

        let mut colour = vec![WHITE; self.modules.len()];
        let mut found: Vec<Vec<String>> = Vec::new();
        let mut truncated = false;

        for root in 0..self.modules.len() {
            if colour[root] != WHITE {
                continue;
            }
            let mut path: Vec<usize> = Vec::new();
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            colour[root] = GREY;
            path.push(root);

            while let Some((current, next)) = stack.pop() {
                if next >= self.modules[current].dependencies.len() {
                    colour[current] = BLACK;
                    path.pop();
                    continue;
                }
                stack.push((current, next + 1));

                let target = self.modules[current].dependencies[next].target;
                match colour[target] {
                    GREY => {
                        if found.len() >= limit {
                            truncated = true;
                            continue;
                        }
                        if let Some(start) = path.iter().position(|&node| node == target) {
                            let mut cycle: Vec<String> = path[start..]
                                .iter()
                                .map(|&node| self.modules[node].relative_path.clone())
                                .collect();
                            cycle.push(self.modules[target].relative_path.clone());
                            if !found.contains(&cycle) {
                                found.push(cycle);
                            }
                        }
                    }
                    WHITE => {
                        colour[target] = GREY;
                        path.push(target);
                        stack.push((target, 0));
                    }
                    _ => {}
                }
            }
        }
        (found, truncated)
    }

    /// Every require in the project that resolved to nothing, keyed by the file that wrote it.
    pub fn all_unresolved(&self, limit: usize) -> (Vec<(String, UnresolvedRequire)>, bool) {
        let mut found = Vec::new();
        let mut truncated = false;
        for module in &self.modules {
            for entry in &module.unresolved {
                if found.len() >= limit {
                    truncated = true;
                    return (found, truncated);
                }
                found.push((module.relative_path.clone(), entry.clone()));
            }
        }
        (found, truncated)
    }
}

/// An unresolved require, told alongside the file that wrote it.
#[derive(Debug, Clone, Serialize)]
pub struct UnresolvedEntry {
    pub relative_path: String,
    pub line: u32,
    pub expression: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphAnswer {
    /// The module the graph was centred on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_path: Option<String>,
    pub modules_scanned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<ReachedModule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependents: Option<Vec<ReachedModule>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedEntry>,
    /// Each cycle as the loop it closes, first module repeated at the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycles: Option<Vec<Vec<String>>>,
    #[serde(skip_serializing_if = "crate::serde_skip::is_false")]
    pub truncated: bool,
    pub sourcemap: super::super::sourcemap::SourcemapReference,
}

#[derive(Debug, Clone)]
pub struct GraphRequest<'a> {
    pub relative_path: Option<&'a str>,
    pub directions: Vec<Direction>,
    pub depth: u32,
    pub include_cycles: bool,
    pub include_unresolved: bool,
    pub limit: usize,
}

impl RequireGraph {
    pub fn answer(&self, request: GraphRequest<'_>, sourcemap: &Sourcemap) -> Result<GraphAnswer> {
        let mut truncated = false;

        let (relative_path, instance_path, dependencies, dependents, unresolved) =
            match request.relative_path {
                Some(relative) => {
                    let index = self.find(relative).ok_or_else(|| {
                        crate::errors::hinted(
                            format!("no Luau file at {relative}"),
                            "pass a path to a .luau or .lua file relative to the project root, or \
                             omit relative_path for a project-wide answer",
                        )
                    })?;
                    let module = self.module(index);

                    let mut walk = |direction: Direction| {
                        request.directions.contains(&direction).then(|| {
                            let (reached, cut) =
                                self.walk(index, direction, request.depth, request.limit);
                            truncated |= cut;
                            reached
                        })
                    };
                    let dependencies = walk(Direction::Dependencies);
                    let dependents = walk(Direction::Dependents);

                    let unresolved = match request.include_unresolved {
                        true => module
                            .unresolved
                            .iter()
                            .map(|entry| UnresolvedEntry {
                                relative_path: module.relative_path.clone(),
                                line: entry.line,
                                expression: entry.expression.clone(),
                                reason: entry.reason.clone(),
                            })
                            .collect(),
                        false => Vec::new(),
                    };

                    (
                        Some(module.relative_path.clone()),
                        module.instance_path.clone(),
                        dependencies,
                        dependents,
                        unresolved,
                    )
                }
                None => {
                    let unresolved = match request.include_unresolved {
                        true => {
                            let (found, cut) = self.all_unresolved(request.limit);
                            truncated |= cut;
                            found
                                .into_iter()
                                .map(|(relative_path, entry)| UnresolvedEntry {
                                    relative_path,
                                    line: entry.line,
                                    expression: entry.expression,
                                    reason: entry.reason,
                                })
                                .collect()
                        }
                        false => Vec::new(),
                    };
                    (None, None, None, None, unresolved)
                }
            };

        let cycles = request.include_cycles.then(|| {
            let (found, cut) = self.cycles(request.limit);
            truncated |= cut;
            found
        });

        Ok(GraphAnswer {
            relative_path,
            instance_path,
            modules_scanned: self.len(),
            dependencies,
            dependents,
            unresolved,
            cycles,
            truncated,
            sourcemap: sourcemap.reference(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::Direction;
    use super::super::build::tests::fixture;

    #[test]
    fn dependents_are_the_edges_read_backwards() {
        let (_dir, _project, graph) = fixture();
        let index = graph.find("src/Shared/C.luau").unwrap();
        let (reached, _) = graph.walk(index, Direction::Dependents, 1, 100);

        let mut paths: Vec<String> = reached
            .into_iter()
            .map(|module| module.relative_path)
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "src/Shared/A.luau".to_string(),
                "src/Shared/B.luau".to_string()
            ]
        );
    }

    #[test]
    fn a_cycle_is_reported_as_the_loop_it_closes() {
        let (_dir, _project, graph) = fixture();
        let (cycles, truncated) = graph.cycles(10);
        assert!(!truncated);
        assert!(!cycles.is_empty(), "the fixture requires in a circle");

        let closed = cycles
            .iter()
            .find(|cycle| cycle.contains(&"src/Shared/C.luau".to_string()))
            .expect("C is in a cycle");
        assert_eq!(
            closed.first(),
            closed.last(),
            "a cycle names the module it comes back to: {closed:?}"
        );
    }
}
