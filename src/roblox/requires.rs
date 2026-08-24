use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use anyhow::Result;
use regex::Regex;
use serde::Serialize;

use super::shared_require::{self, Resolution, SharedIndex};
use super::sourcemap::Sourcemap;
use crate::config::Settings;
use crate::lines::LineIndex;
use crate::project::{self, Project};

const CHILD_BY_NAME_METHODS: [&str; 3] = ["GetService", "WaitForChild", "FindFirstChild"];

const MODULE_SUFFIXES: [&str; 4] = [".luau", ".lua", "/init.luau", "/init.lua"];

/// Which way an edge is followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Dependencies,
    Dependents,
}

impl Direction {
    pub fn parse(value: Option<&str>) -> Result<Vec<Self>> {
        match value.unwrap_or("both").trim() {
            "both" => Ok(vec![Self::Dependencies, Self::Dependents]),
            "dependencies" => Ok(vec![Self::Dependencies]),
            "dependents" => Ok(vec![Self::Dependents]),
            other => Err(crate::errors::hinted(
                format!(
                    "direction must be \"dependencies\", \"dependents\", or \"both\", got {other:?}"
                ),
                "omit direction to get both",
            )),
        }
    }
}

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

/// What the file set looked like when a graph was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphStamp {
    sourcemap: Option<(SystemTime, u64)>,
    files: usize,
    total_len: u64,
    newest: Option<SystemTime>,
}

#[derive(Debug)]
pub struct RequireGraph {
    modules: Vec<Module>,
    index: HashMap<String, usize>,
    stamp: GraphStamp,
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
            .get(&super::sourcemap::normalize(relative_path))
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
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
    pub sourcemap: super::sourcemap::SourcemapReference,
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

/// Rebuilds the graph only when the files it was built from have moved.
pub fn build_or_reuse(
    cached: Option<std::sync::Arc<RequireGraph>>,
    project: &Project,
    settings: &Settings,
    sourcemap: &Sourcemap,
) -> Result<std::sync::Arc<RequireGraph>> {
    let files = luau_files(project, settings)?;
    let stamp = stamp_of(&files, sourcemap);

    if let Some(existing) = cached.filter(|graph| graph.stamp() == &stamp) {
        return Ok(existing);
    }
    Ok(std::sync::Arc::new(build(
        project, settings, sourcemap, files, stamp,
    )?))
}

fn luau_files(project: &Project, settings: &Settings) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in project::walk_builder(project.root(), &settings.project)?
        .build()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("luau" | "lua")
        ) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

fn stamp_of(files: &[PathBuf], sourcemap: &Sourcemap) -> GraphStamp {
    let mut total_len = 0;
    let mut newest: Option<SystemTime> = None;
    for path in files {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        total_len += metadata.len();
        if let Ok(modified) = metadata.modified() {
            newest = Some(newest.map_or(modified, |current: SystemTime| current.max(modified)));
        }
    }
    GraphStamp {
        sourcemap: sourcemap.stamp(),
        files: files.len(),
        total_len,
        newest,
    }
}

fn build(
    project: &Project,
    settings: &Settings,
    sourcemap: &Sourcemap,
    files: Vec<PathBuf>,
    stamp: GraphStamp,
) -> Result<RequireGraph> {
    let mut modules: Vec<Module> = Vec::with_capacity(files.len());
    let mut index: HashMap<String, usize> = HashMap::with_capacity(files.len());
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());

    for path in files {
        let Ok(relative) = project.relativize(&path) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let node = sourcemap.nodes_for_file(&relative).first().copied();
        index.insert(relative.clone(), modules.len());
        modules.push(Module {
            instance_path: node.map(|node| sourcemap.node(node).instance_path.clone()),
            relative_path: relative,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            unresolved: Vec::new(),
        });
        sources.push((path, source));
    }

    let mut aliases = AliasCache::new(project);

    let shared = settings.project.shared_require.then(|| {
        let paths: Vec<String> = modules
            .iter()
            .map(|module| module.relative_path.clone())
            .collect();
        SharedIndex::build(&paths, sourcemap)
    });

    for (owner, (path, source)) in sources.iter().enumerate() {
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let environment = local_bindings(&blanked);
        let script_node = sourcemap
            .nodes_for_file(&modules[owner].relative_path)
            .first()
            .copied();

        let scan_shared =
            shared.is_some() && !environment.contains_key(shared_require::GLOBAL_NAME);

        for call in find_calls(&blanked, &lines, scan_shared) {
            let resolution = match &call.kind {
                CallKind::Require => resolve(
                    &call.expression,
                    &environment,
                    sourcemap,
                    script_node,
                    path,
                    project,
                    &mut aliases,
                ),
                CallKind::Shared(name) => resolve_shared(
                    name.as_deref(),
                    shared.as_ref(),
                    &modules[owner].relative_path,
                ),
            };

            match resolution {
                Ok(relative) => match index.get(&relative) {
                    Some(&target) if target != owner => {
                        modules[owner].dependencies.push(Edge {
                            target,
                            line: call.line,
                            expression: call.expression.clone(),
                        });
                        modules[target].dependents.push(owner);
                    }
                    Some(_) => {}
                    None => modules[owner].unresolved.push(UnresolvedRequire {
                        line: call.line,
                        expression: call.expression.clone(),
                        reason: format!(
                            "resolves to {relative}, which is outside the files Biskit scans"
                        ),
                    }),
                },
                Err(reason) => modules[owner].unresolved.push(UnresolvedRequire {
                    line: call.line,
                    expression: call.expression.clone(),
                    reason,
                }),
            }
        }
    }

    for module in &mut modules {
        module.dependents.sort_unstable();
        module.dependents.dedup();
    }

    Ok(RequireGraph {
        modules,
        index,
        stamp,
    })
}

enum CallKind {
    Require,
    Shared(Option<String>),
}

struct RequireCall {
    line: u32,
    offset: usize,
    expression: String,
    kind: CallKind,
}

fn find_calls(blanked: &str, lines: &LineIndex<'_>, include_shared: bool) -> Vec<RequireCall> {
    let mut found = find_requires(blanked, lines);
    if include_shared {
        found.extend(find_shared_requires(blanked, lines));
        found.sort_by_key(|call| call.offset);
    }
    found
}

fn find_requires(blanked: &str, lines: &LineIndex<'_>) -> Vec<RequireCall> {
    let bytes = blanked.as_bytes();
    let mut found = Vec::new();
    let finder = memchr::memmem::Finder::new(b"require");

    let mut from = 0;
    while let Some(offset) = finder.find(&bytes[from..]) {
        let start = from + offset;
        from = start + 7;

        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        if !before_ok {
            continue;
        }

        let mut cursor = start + 7;
        while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            continue;
        }

        let expression = match bytes[cursor] {
            b'(' => match balanced(bytes, cursor) {
                Some((inner, end)) => {
                    from = end;
                    blanked[inner].trim().to_string()
                }
                None => continue,
            },
            b'"' | b'\'' => {
                let rest = &blanked[cursor..];
                let quote = bytes[cursor] as char;
                match rest[1..].find(quote) {
                    Some(end) => rest[..end + 2].to_string(),
                    None => continue,
                }
            }
            _ => continue,
        };

        if expression.is_empty() {
            continue;
        }
        found.push(RequireCall {
            line: lines.line_of(start) as u32 + 1,
            offset: start,
            expression,
            kind: CallKind::Require,
        });
    }
    found
}

fn find_shared_requires(blanked: &str, lines: &LineIndex<'_>) -> Vec<RequireCall> {
    let bytes = blanked.as_bytes();
    let name = shared_require::GLOBAL_NAME;
    let mut found = Vec::new();
    let finder = memchr::memmem::Finder::new(name.as_bytes());

    let mut from = 0;
    while let Some(offset) = finder.find(&bytes[from..]) {
        let start = from + offset;
        from = start + name.len();

        if start > 0 {
            let before = bytes[start - 1];
            if is_word_byte(before) || before == b'.' || before == b':' {
                continue;
            }
        }

        let mut cursor = start + name.len();
        if cursor < bytes.len() && is_word_byte(bytes[cursor]) {
            continue;
        }
        while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            continue;
        }

        let expression = match bytes[cursor] {
            b'(' => match balanced(bytes, cursor) {
                Some((inner, end)) => {
                    from = end;
                    blanked[inner].trim().to_string()
                }
                None => continue,
            },
            b'"' | b'\'' => {
                let rest = &blanked[cursor..];
                let quote = bytes[cursor] as char;
                match rest[1..].find(quote) {
                    Some(end) => rest[..end + 2].to_string(),
                    None => continue,
                }
            }
            _ => continue,
        };

        if expression.is_empty() {
            continue;
        }
        let module = string_literal(&expression);
        found.push(RequireCall {
            line: lines.line_of(start) as u32 + 1,
            offset: start,
            expression,
            kind: CallKind::Shared(module),
        });
    }
    found
}

fn string_literal(expression: &str) -> Option<String> {
    let chars: Vec<char> = expression.chars().collect();
    let quote = *chars.first()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let mut value = String::new();
    let mut index = 1;
    while index < chars.len() {
        match chars[index] {
            character if character == quote => {
                return chars[index + 1..]
                    .iter()
                    .all(|rest| rest.is_whitespace())
                    .then_some(value);
            }
            '\\' => {
                let escaped = *chars.get(index + 1)?;
                value.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                index += 2;
            }
            character => {
                value.push(character);
                index += 1;
            }
        }
    }
    None
}

fn balanced(bytes: &[u8], open: usize) -> Option<(std::ops::Range<usize>, usize)> {
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1..index, index + 1));
                }
            }
            quote @ (b'"' | b'\'') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn local_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*local[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*([^\r\n]+)")
            .expect("the local binding pattern is a literal")
    })
}

fn local_bindings(blanked: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    for capture in local_pattern().captures_iter(blanked) {
        bindings.insert(capture[1].to_string(), capture[2].trim().to_string());
    }
    bindings
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Child(String),
    Parent,
}

struct Chain {
    head: String,
    steps: Vec<Step>,
}

fn parse_chain(expression: &str) -> Result<Chain, String> {
    let chars: Vec<char> = expression.chars().collect();
    let mut index = 0;

    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let (head, next) = read_identifier(&chars, index);
    if head.is_empty() {
        return Err("not an instance expression".to_string());
    }
    index = next;

    let mut steps = Vec::new();
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index >= chars.len() {
            return Ok(Chain { head, steps });
        }

        match chars[index] {
            '.' => {
                let (name, next) = read_identifier(&chars, index + 1);
                if name.is_empty() {
                    return Err("not an instance expression".to_string());
                }
                index = next;
                steps.push(match name.as_str() {
                    "Parent" => Step::Parent,
                    _ => Step::Child(name),
                });
            }
            ':' => {
                let (method, next) = read_identifier(&chars, index + 1);
                let (argument, next) = read_call_argument(&chars, next);
                index = next;
                match argument {
                    Some(name) if CHILD_BY_NAME_METHODS.contains(&method.as_str()) => {
                        steps.push(Step::Child(name))
                    }
                    _ => return Err(format!("cannot resolve :{method}(...) statically")),
                }
            }
            '[' => {
                let (argument, next) = read_bracket_argument(&chars, index);
                index = next;
                match argument {
                    Some(name) => steps.push(Step::Child(name)),
                    None => return Err("indexed by a value, not a literal name".to_string()),
                }
            }
            '(' => return Err("the require argument is a call result".to_string()),
            _ => return Err("not an instance expression".to_string()),
        }
    }
}

fn read_identifier(chars: &[char], from: usize) -> (String, usize) {
    let mut index = from;
    let mut identifier = String::new();
    while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_') {
        identifier.push(chars[index]);
        index += 1;
    }
    (identifier, index)
}

fn read_call_argument(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from;
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    if index >= chars.len() || chars[index] != '(' {
        return (None, index);
    }

    let mut depth = 0usize;
    let mut literal = None;
    let mut only_literal = true;
    while index < chars.len() {
        match chars[index] {
            '(' => {
                depth += 1;
                index += 1;
            }
            ')' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return (literal.filter(|_| only_literal), index);
                }
            }
            quote @ ('"' | '\'') => {
                let (text, next) = read_string(chars, index, quote);
                if literal.is_none() {
                    literal = Some(text);
                }
                index = next;
            }
            character if character.is_whitespace() || character == ',' => index += 1,
            _ => {
                if literal.is_none() {
                    only_literal = false;
                }
                index += 1;
            }
        }
    }
    (None, index)
}

fn read_bracket_argument(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from + 1;
    let mut literal = None;
    let mut only_literal = true;
    while index < chars.len() {
        match chars[index] {
            ']' => return (literal.filter(|_| only_literal), index + 1),
            quote @ ('"' | '\'') => {
                let (text, next) = read_string(chars, index, quote);
                if literal.is_none() {
                    literal = Some(text);
                }
                index = next;
            }
            character if character.is_whitespace() => index += 1,
            _ => {
                only_literal = false;
                index += 1;
            }
        }
    }
    (None, index)
}

fn read_string(chars: &[char], from: usize, quote: char) -> (String, usize) {
    let mut index = from + 1;
    let mut text = String::new();
    while index < chars.len() {
        match chars[index] {
            '\\' if index + 1 < chars.len() => {
                text.push(chars[index + 1]);
                index += 2;
            }
            character if character == quote => return (text, index + 1),
            character => {
                text.push(character);
                index += 1;
            }
        }
    }
    (text, index)
}

fn resolve_shared(
    name: Option<&str>,
    index: Option<&SharedIndex>,
    requiring_relative_path: &str,
) -> Result<String, String> {
    let Some(name) = name else {
        return Err(
            "a shared() call whose argument is not a string literal, which the language \
                    server does not resolve either"
                .to_string(),
        );
    };
    let Some(index) = index else {
        return Err("shared() resolution is off; set project.shared_require to true".to_string());
    };

    match index.resolve(name, requiring_relative_path) {
        Resolution::Found(relative) => Ok(relative),
        Resolution::NotFound => Err(format!("shared({name:?}) names no module in the project")),
        Resolution::Ambiguous(candidates) => Err(format!(
            "shared({name:?}) is ambiguous: it matches {}, and none of them is nearer than the \
             rest",
            candidates.join(", ")
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve(
    expression: &str,
    environment: &HashMap<String, String>,
    sourcemap: &Sourcemap,
    script_node: Option<usize>,
    path: &Path,
    project: &Project,
    aliases: &mut AliasCache,
) -> Result<String, String> {
    let trimmed = expression.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let chars: Vec<char> = trimmed.chars().collect();
        let quote = chars[0];
        let (literal, _) = read_string(&chars, 0, quote);
        return resolve_string_require(&literal, path, project, aliases);
    }

    let node = resolve_instance(trimmed, environment, sourcemap, script_node, 0)?;
    match sourcemap.script_file(node) {
        Some(file) => Ok(file.to_string()),
        None => Err(format!(
            "resolves to {}, which is a {} rather than a script",
            sourcemap.node(node).instance_path,
            sourcemap.node(node).class_name
        )),
    }
}

const MAX_BINDING_HOPS: usize = 8;

fn resolve_instance(
    expression: &str,
    environment: &HashMap<String, String>,
    sourcemap: &Sourcemap,
    script_node: Option<usize>,
    hops: usize,
) -> Result<usize, String> {
    if hops > MAX_BINDING_HOPS {
        return Err("the local bindings it is built from refer to each other".to_string());
    }

    let chain = parse_chain(expression)?;
    let mut current = match chain.head.as_str() {
        "script" => {
            script_node.ok_or_else(|| "the requiring file is not in the sourcemap".to_string())?
        }
        "game" => sourcemap.root(),
        "workspace" => sourcemap
            .child(sourcemap.root(), "Workspace")
            .ok_or_else(|| "the sourcemap has no Workspace".to_string())?,
        head => match environment.get(head) {
            Some(bound) => resolve_instance(bound, environment, sourcemap, script_node, hops + 1)?,
            None => sourcemap
                .child(sourcemap.root(), head)
                .ok_or_else(|| format!("{head} is not a known instance in this file"))?,
        },
    };

    for step in chain.steps {
        current = match step {
            Step::Parent => sourcemap.parent(current).ok_or_else(|| {
                format!(
                    "{} has no parent, so the require walks past the DataModel",
                    sourcemap.node(current).instance_path
                )
            })?,
            Step::Child(name) => sourcemap.child(current, &name).ok_or_else(|| {
                format!(
                    "no instance named {name} under {}",
                    sourcemap.node(current).instance_path
                )
            })?,
        };
    }
    Ok(current)
}

fn resolve_string_require(
    literal: &str,
    path: &Path,
    project: &Project,
    aliases: &mut AliasCache,
) -> Result<String, String> {
    let directory = path
        .parent()
        .ok_or_else(|| "the requiring file has no directory".to_string())?;

    let base = if let Some(rest) = literal.strip_prefix('@') {
        let (alias, remainder) = match rest.split_once('/') {
            Some((alias, remainder)) => (alias, remainder),
            None => (rest, ""),
        };
        let resolved = aliases
            .lookup(directory, alias)
            .ok_or_else(|| format!("no .luaurc alias named @{alias} applies to this file"))?;
        match remainder.is_empty() {
            true => resolved,
            false => resolved.join(remainder),
        }
    } else if literal.starts_with("./") || literal.starts_with("../") {
        directory.join(literal)
    } else {
        return Err("a string require that is neither an alias nor a relative path".to_string());
    };

    for suffix in MODULE_SUFFIXES {
        let candidate = PathBuf::from(format!("{}{suffix}", base.display()));
        if candidate.is_file()
            && let Ok(relative) = project.relativize(&normalize_path(&candidate))
        {
            return Ok(relative);
        }
    }
    Err(format!("{literal} does not name a file on disk"))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other),
        }
    }
    cleaned
}

struct AliasCache {
    root: PathBuf,
    by_directory: HashMap<PathBuf, HashMap<String, PathBuf>>,
}

impl AliasCache {
    fn new(project: &Project) -> Self {
        Self {
            root: project.root().to_path_buf(),
            by_directory: HashMap::new(),
        }
    }

    fn lookup(&mut self, directory: &Path, alias: &str) -> Option<PathBuf> {
        let mut current = Some(directory);
        while let Some(here) = current {
            if !here.starts_with(&self.root) {
                break;
            }
            let aliases = match self.by_directory.get(here) {
                Some(found) => found,
                None => {
                    let loaded = read_aliases(here);
                    self.by_directory.insert(here.to_path_buf(), loaded);
                    &self.by_directory[here]
                }
            };
            if let Some(target) = aliases.get(alias) {
                return Some(here.join(target));
            }
            current = here.parent();
        }
        None
    }
}

fn read_aliases(directory: &Path) -> HashMap<String, PathBuf> {
    let Ok(raw) = std::fs::read_to_string(directory.join(".luaurc")) else {
        return HashMap::new();
    };
    let stripped: String = raw
        .lines()
        .map(|line| match line.trim_start().starts_with("//") {
            true => "",
            false => line,
        })
        .collect::<Vec<&str>>()
        .join("\n");

    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stripped) else {
        return HashMap::new();
    };
    let Some(aliases) = parsed.get("aliases").and_then(|value| value.as_object()) else {
        return HashMap::new();
    };
    aliases
        .iter()
        .filter_map(|(alias, target)| Some((alias.clone(), PathBuf::from(target.as_str()?))))
        .collect()
}

/// Blanks every comment, keeping every byte offset and every line break where it was.
pub fn blank_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                let start = index;
                index += 2;
                match long_bracket(bytes, index) {
                    Some(level) => {
                        let close = format!("]{}]", "=".repeat(level));
                        let from = index + level + 2;
                        let end = source[from..]
                            .find(&close)
                            .map(|offset| from + offset + close.len())
                            .unwrap_or(bytes.len());
                        blank(&mut out, start, end);
                        index = end;
                    }
                    None => {
                        let end = source[index..]
                            .find('\n')
                            .map(|offset| index + offset)
                            .unwrap_or(bytes.len());
                        blank(&mut out, start, end);
                        index = end;
                    }
                }
            }
            quote @ (b'"' | b'\'') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote && bytes[index] != b'\n' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
            b'[' => match long_bracket(bytes, index) {
                Some(level) => {
                    let close = format!("]{}]", "=".repeat(level));
                    let from = index + level + 2;
                    index = source[from..]
                        .find(&close)
                        .map(|offset| from + offset + close.len())
                        .unwrap_or(bytes.len());
                }
                None => index += 1,
            },
            _ => index += 1,
        }
    }

    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

fn long_bracket(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut level = 0;
    while bytes.get(index + 1 + level) == Some(&b'=') {
        level += 1;
    }
    match bytes.get(index + 1 + level) {
        Some(&b'[') => Some(level),
        _ => None,
    }
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    let end = to.min(out.len());
    for byte in &mut out[from..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    const SOURCEMAP: &str = r#"{
        "name": "Fixture",
        "className": "DataModel",
        "children": [{
            "name": "ReplicatedStorage",
            "className": "ReplicatedStorage",
            "children": [{
                "name": "Shared",
                "className": "Folder",
                "filePaths": ["src/Shared"],
                "children": [
                    {"name": "A", "className": "ModuleScript", "filePaths": ["src/Shared/A.luau"]},
                    {"name": "B", "className": "ModuleScript", "filePaths": ["src/Shared/B.luau"]},
                    {"name": "C", "className": "ModuleScript", "filePaths": ["src/Shared/C.luau"]},
                    {"name": "Assets", "className": "Folder", "filePaths": ["src/Shared/Assets"]}
                ]
            }]
        }]
    }"#;

    fn fixture() -> (tempfile::TempDir, Project, RequireGraph) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Shared/Assets")).unwrap();
        std::fs::create_dir_all(root.join("Packages")).unwrap();
        std::fs::write(root.join("sourcemap.json"), SOURCEMAP).unwrap();
        std::fs::write(
            root.join(".luaurc"),
            "{\n// vendored by wally\n\"aliases\": {\"Packages\": \"Packages\"}\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("Packages/Promise.luau"), "return {}\n").unwrap();

        std::fs::write(
            root.join("src/Shared/A.luau"),
            "local ReplicatedStorage = game:GetService(\"ReplicatedStorage\")\n\
             local Shared = ReplicatedStorage:WaitForChild(\"Shared\")\n\
             local B = require(script.Parent.B)\n\
             local C = require(Shared.C)\n\
             local Promise = require(\"@Packages/Promise\")\n\
             local dynamic = require(Shared[name])\n\
             local folder = require(Shared.Assets)\n\
             return {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Shared/B.luau"),
            "local C = require(game.ReplicatedStorage.Shared.C)\nreturn {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Shared/C.luau"),
            "local A = require(script.Parent.A)\nreturn {}\n",
        )
        .unwrap();

        let project = Project::open(root).unwrap();
        let settings = Settings::default();
        let sourcemap = Sourcemap::load(&project, &settings).unwrap();
        let files = luau_files(&project, &settings).unwrap();
        let stamp = stamp_of(&files, &sourcemap);
        let graph = build(&project, &settings, &sourcemap, files, stamp).unwrap();
        (dir, project, graph)
    }

    fn dependency_paths(graph: &RequireGraph, relative: &str) -> Vec<String> {
        let index = graph.find(relative).expect("module is in the graph");
        let (reached, _) = graph.walk(index, Direction::Dependencies, 1, 100);
        let mut paths: Vec<String> = reached
            .into_iter()
            .map(|module| module.relative_path)
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn every_shape_of_require_a_roblox_module_writes_resolves_to_the_same_graph() {
        let (_dir, _project, graph) = fixture();
        assert_eq!(
            dependency_paths(&graph, "src/Shared/A.luau"),
            vec![
                "Packages/Promise.luau".to_string(),
                "src/Shared/B.luau".to_string(),
                "src/Shared/C.luau".to_string(),
            ]
        );
    }

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
    fn a_require_that_cannot_be_resolved_is_reported_rather_than_dropped() {
        let (_dir, _project, graph) = fixture();
        let module = graph.module(graph.find("src/Shared/A.luau").unwrap());

        let reasons: Vec<(&str, &str)> = module
            .unresolved
            .iter()
            .map(|entry| (entry.expression.as_str(), entry.reason.as_str()))
            .collect();
        assert_eq!(reasons.len(), 2, "unexpected: {reasons:?}");

        let dynamic = reasons
            .iter()
            .find(|(expression, _)| expression.contains("[name]"))
            .expect("the dynamic index is reported");
        assert!(dynamic.1.contains("not a literal name"));

        let folder = reasons
            .iter()
            .find(|(expression, _)| expression.contains("Assets"))
            .expect("the folder require is reported");
        assert!(folder.1.contains("Folder"), "unexpected: {}", folder.1);
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

    #[test]
    fn an_unchanged_project_reuses_the_graph_it_already_built() {
        let (dir, project, graph) = fixture();
        let settings = Settings::default();
        let sourcemap = Sourcemap::load(&project, &settings).unwrap();

        let cached = std::sync::Arc::new(graph);
        let again =
            build_or_reuse(Some(Arc::clone(&cached)), &project, &settings, &sourcemap).unwrap();
        assert!(Arc::ptr_eq(&cached, &again), "the graph was rebuilt");

        std::fs::write(
            dir.path().join("src/Shared/B.luau"),
            "-- changed\nreturn {}\n",
        )
        .unwrap();
        let rebuilt =
            build_or_reuse(Some(Arc::clone(&cached)), &project, &settings, &sourcemap).unwrap();
        assert!(
            !Arc::ptr_eq(&cached, &rebuilt),
            "an edited file did not invalidate the graph"
        );
        assert!(dependency_paths(&rebuilt, "src/Shared/B.luau").is_empty());
    }

    #[test]
    fn a_commented_out_require_is_not_a_dependency() {
        let source = "-- require(script.Parent.Old)\nrequire(script.Parent.New)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].expression, "script.Parent.New");
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn a_long_comment_is_blanked_without_moving_the_lines_after_it() {
        let source = "--[==[\nrequire(script.Hidden)\n]==]\nrequire(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 4);
    }

    #[test]
    fn a_double_dash_inside_a_string_does_not_start_a_comment() {
        let source = "local separator = \"--\" local m = require(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert_eq!(find_requires(&blanked, &lines).len(), 1);
    }

    #[test]
    fn a_require_spanning_lines_is_read_whole() {
        let source = "local m = require(\n\tgame:GetService(\"ReplicatedStorage\").Shared\n)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert!(found[0].expression.contains("GetService"));
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn a_word_ending_in_require_is_not_a_require() {
        let source = "local prerequire = 1\nlocal m = require(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert_eq!(find_requires(&blanked, &lines).len(), 1);
    }

    #[test]
    fn chains_parse_into_the_traversal_they_describe() {
        let chain = parse_chain("script.Parent.Parent.Shared.Combat").unwrap();
        assert_eq!(chain.head, "script");
        assert_eq!(
            chain.steps,
            vec![
                Step::Parent,
                Step::Parent,
                Step::Child("Shared".to_string()),
                Step::Child("Combat".to_string()),
            ]
        );

        let waited = parse_chain("ReplicatedStorage:WaitForChild(\"Shared\").Combat").unwrap();
        assert_eq!(waited.head, "ReplicatedStorage");
        assert_eq!(
            waited.steps,
            vec![
                Step::Child("Shared".to_string()),
                Step::Child("Combat".to_string()),
            ]
        );
    }

    #[test]
    fn a_chain_that_cannot_be_read_statically_is_refused_rather_than_guessed_at() {
        for expression in [
            "modules[name]",
            "Madwork.GetShared(\"Madwork\", \"MadworkMaid\")",
            "script:FindFirstAncestor(name).Thing",
            "script:WaitForChild(childName)",
        ] {
            assert!(
                parse_chain(expression).is_err(),
                "{expression} should not have parsed"
            );
        }
    }

    #[test]
    fn local_bindings_keep_the_last_value_a_name_was_given() {
        let bindings = local_bindings("local a = script.One\nlocal a = script.Two\n");
        assert_eq!(bindings.get("a").map(String::as_str), Some("script.Two"));
    }

    #[test]
    fn multiple_assignment_is_not_read_as_a_binding() {
        let bindings = local_bindings("local a, b = 1, 2\n");
        assert!(bindings.is_empty());
    }

    const SHARED_SOURCEMAP: &str = r#"{
        "name": "Fixture",
        "className": "DataModel",
        "children": [
            {
                "name": "ReplicatedStorage",
                "className": "ReplicatedStorage",
                "children": [{
                    "name": "Shared",
                    "className": "Folder",
                    "filePaths": ["src/Shared"],
                    "children": [
                        {"name": "Consumer", "className": "ModuleScript",
                         "filePaths": ["src/Shared/Consumer.luau"]},
                        {"name": "Config", "className": "ModuleScript",
                         "filePaths": ["src/Shared/Config.luau"]}
                    ]
                }]
            },
            {
                "name": "ServerScriptService",
                "className": "ServerScriptService",
                "children": [{
                    "name": "Server",
                    "className": "Folder",
                    "filePaths": ["src/Server"],
                    "children": [
                        {"name": "Config", "className": "ModuleScript",
                         "filePaths": ["src/Server/Config.luau"]}
                    ]
                }]
            }
        ]
    }"#;

    fn shared_fixture(shared_require: bool) -> (tempfile::TempDir, RequireGraph) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Shared")).unwrap();
        std::fs::create_dir_all(root.join("src/Server")).unwrap();
        std::fs::write(root.join("sourcemap.json"), SHARED_SOURCEMAP).unwrap();

        std::fs::write(
            root.join("src/Shared/Consumer.luau"),
            "local Config = shared(\"Config\")\n\
             local Server = shared(\"Server/Config\")\n\
             local bare = shared \"Config\"\n\
             local dynamic = shared(name)\n\
             local joined = shared(\"Con\" .. \"fig\")\n\
             local missing = shared(\"Nonexistent\")\n\
             shared.someFlag = true\n\
             local held = shared.cache\n\
             return {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/Shared/Config.luau"), "return {}\n").unwrap();
        std::fs::write(root.join("src/Server/Config.luau"), "return {}\n").unwrap();

        let project = Project::open(root).unwrap();
        let settings = Settings {
            project: crate::config::ProjectSettings {
                shared_require,
                ..Default::default()
            },
            ..Default::default()
        };
        let sourcemap = Sourcemap::load(&project, &settings).unwrap();
        let files = luau_files(&project, &settings).unwrap();
        let stamp = stamp_of(&files, &sourcemap);
        let graph = build(&project, &settings, &sourcemap, files, stamp).unwrap();
        (dir, graph)
    }

    fn unresolved_reasons(graph: &RequireGraph, relative: &str) -> Vec<String> {
        let index = graph.find(relative).expect("module is in the graph");
        graph
            .module(index)
            .unresolved
            .iter()
            .map(|entry| entry.reason.clone())
            .collect()
    }

    #[test]
    fn a_shared_call_is_an_edge_like_any_other_require() {
        let (_dir, graph) = shared_fixture(true);
        assert_eq!(
            dependency_paths(&graph, "src/Shared/Consumer.luau"),
            vec![
                "src/Server/Config.luau".to_string(),
                "src/Shared/Config.luau".to_string(),
            ]
        );

        let config = graph
            .find("src/Shared/Config.luau")
            .expect("the target is in the graph");
        let (dependents, _) = graph.walk(config, Direction::Dependents, 1, 100);
        assert_eq!(
            dependents
                .iter()
                .map(|module| module.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/Shared/Consumer.luau"],
            "the edge is readable backwards too"
        );
    }

    #[test]
    fn an_unqualified_shared_name_resolves_to_the_nearest_candidate() {
        let (_dir, graph) = shared_fixture(true);
        let consumer = graph.find("src/Shared/Consumer.luau").unwrap();
        let edge = graph
            .module(consumer)
            .dependencies
            .iter()
            .find(|edge| edge.expression == "\"Config\"" && edge.line == 1)
            .expect("the first shared call is an edge");
        assert_eq!(
            graph.module(edge.target).relative_path,
            "src/Shared/Config.luau"
        );
    }

    #[test]
    fn a_shared_call_biskit_cannot_read_is_reported_rather_than_dropped() {
        let (_dir, graph) = shared_fixture(true);
        let reasons = unresolved_reasons(&graph, "src/Shared/Consumer.luau");

        assert_eq!(
            reasons
                .iter()
                .filter(|reason| reason.contains("not a string literal"))
                .count(),
            2,
            "the variable and the concatenation are both reported: {reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("\"Nonexistent\"") && reason.contains("no module")),
            "unexpected reasons: {reasons:?}"
        );
    }

    #[test]
    fn indexing_the_shared_table_is_not_a_require() {
        let source = "shared.someFlag = true\nlocal held = shared.cache\nlocal n = shared[key]\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert!(find_shared_requires(&blanked, &lines).is_empty());
    }

    #[test]
    fn a_call_without_parentheses_is_still_a_shared_require() {
        let (_dir, graph) = shared_fixture(true);
        let consumer = graph.find("src/Shared/Consumer.luau").unwrap();
        assert!(
            graph
                .module(consumer)
                .dependencies
                .iter()
                .any(|edge| edge.line == 3),
            "the paren-less call on line 3 is an edge: {:?}",
            graph.module(consumer).dependencies
        );
    }

    #[test]
    fn turning_shared_require_off_leaves_the_calls_alone() {
        let (_dir, graph) = shared_fixture(false);
        assert!(
            dependency_paths(&graph, "src/Shared/Consumer.luau").is_empty(),
            "no shared call should become an edge"
        );
        assert!(
            unresolved_reasons(&graph, "src/Shared/Consumer.luau").is_empty(),
            "and none should be reported as a failure either"
        );
    }

    #[test]
    fn a_shared_the_file_bound_itself_is_not_the_global() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Shared")).unwrap();
        std::fs::write(root.join("sourcemap.json"), SHARED_SOURCEMAP).unwrap();
        std::fs::write(
            root.join("src/Shared/Consumer.luau"),
            "local shared = require(script.Parent.Config)\nlocal x = shared(\"Config\")\nreturn {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/Shared/Config.luau"), "return {}\n").unwrap();

        let project = Project::open(root).unwrap();
        let settings = Settings::default();
        let sourcemap = Sourcemap::load(&project, &settings).unwrap();
        let files = luau_files(&project, &settings).unwrap();
        let stamp = stamp_of(&files, &sourcemap);
        let graph = build(&project, &settings, &sourcemap, files, stamp).unwrap();

        assert_eq!(
            dependency_paths(&graph, "src/Shared/Consumer.luau"),
            vec!["src/Shared/Config.luau".to_string()],
            "only the real require is an edge, and it is not counted twice"
        );
    }

    #[test]
    fn a_qualified_call_is_not_the_shared_global() {
        let source = "local a = Framework.shared(\"Config\")\nlocal b = self:shared(\"Config\")\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert!(find_shared_requires(&blanked, &lines).is_empty());
    }

    #[test]
    fn a_commented_out_shared_call_is_not_a_dependency() {
        let source = "-- shared(\"Old\")\nshared(\"New\")\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_shared_requires(&blanked, &lines);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn calls_are_reported_in_the_order_they_are_written() {
        let source = "require(script.A)\nshared(\"B\")\nrequire(script.C)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let lines_found: Vec<u32> = find_calls(&blanked, &lines, true)
            .iter()
            .map(|call| call.line)
            .collect();
        assert_eq!(lines_found, vec![1, 2, 3]);
    }

    #[test]
    fn only_a_lone_string_literal_carries_a_module_name() {
        assert_eq!(string_literal("\"Foo\""), Some("Foo".to_string()));
        assert_eq!(string_literal("'Foo'"), Some("Foo".to_string()));
        assert_eq!(
            string_literal("\"jobs\\\\Foo\""),
            Some("jobs\\Foo".to_string())
        );
        assert_eq!(string_literal("\"Foo\"  "), Some("Foo".to_string()));

        for expression in ["name", "\"a\" .. b", "\"a\", \"b\"", "\"unterminated", ""] {
            assert_eq!(string_literal(expression), None, "{expression} parsed");
        }
    }
}
