use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::bail_hint;
use crate::config::Settings;
use crate::project::Project;

const MISSING_SOURCEMAP_HINT: &str = "generate one with `rojo sourcemap --include-non-scripts \
                                      --watch default.project.json --output sourcemap.json`, or \
                                      point lsp.sourcemap at the file the project already \
                                      generates";

const DISABLED_SOURCEMAP_HINT: &str = "set lsp.sourcemap in .biskit/settings.yml to this \
                                       project's sourcemap file and restart the server";

/// Children listed when a path resolves partway and then fails, so the caller can correct the
/// segment rather than guess at it again.
const SUGGESTED_CHILDREN: usize = 24;

/// Methods that take the name of a child as their only meaningful argument.
const CHILD_BY_NAME_METHODS: [&str; 4] = [
    "GetService",
    "WaitForChild",
    "FindFirstChild",
    "FindFirstAncestor",
];

/// `sourcemap.json` as rojo writes it: a tree of instances, each naming the files it was built
/// from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNode {
    name: String,
    class_name: String,
    #[serde(default)]
    file_paths: Vec<String>,
    #[serde(default)]
    children: Vec<RawNode>,
}

/// One instance, flattened into an arena so a node can be walked in either direction.
///
/// The tree is read far more often than it is built, and every question Biskit asks of it is
/// either "what is under this" or "what is above this". Parent links are what make the second one
/// answerable at all: a `script.Parent.Parent` require cannot be resolved by descending.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub class_name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Project-relative, with forward slashes whatever platform wrote the sourcemap.
    pub file_paths: Vec<String>,
    /// Spelled out once at load time because it is what almost every answer reports.
    pub instance_path: String,
}

/// What a DataModel-typed answer reports about the sourcemap it came from.
///
/// A sourcemap that has not been regenerated since the last file moved describes a game that no
/// longer exists, and nothing in the answer itself would show it. Reporting when it was written
/// lets the caller judge the answer instead of trusting it.
#[derive(Debug, Clone, Serialize)]
pub struct SourcemapReference {
    pub relative_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_epoch_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
    pub instances: usize,
}

/// One instance, in the form the answer reports it.
#[derive(Debug, Clone, Serialize)]
pub struct InstanceAnswer {
    pub instance_path: String,
    pub class_name: String,
    /// Every file the instance was built from, which for a folder is the directory itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub file_paths: Vec<String>,
    /// The Luau file among them, where there is one. This is the path the symbol tools take.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_file: Option<String>,
    /// The top-level service it sits under, which is what decides where the code runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub children: usize,
}

/// The answer to a translation in either direction.
///
/// A list rather than one instance because a rojo project is free to mount the same directory
/// twice, and answering with the first of two would be answering a question that was not asked.
#[derive(Debug, Clone, Serialize)]
pub struct ResolveAnswer {
    pub instances: Vec<InstanceAnswer>,
    pub sourcemap: SourcemapReference,
}

#[derive(Debug)]
pub struct Sourcemap {
    nodes: Vec<Node>,
    /// Every file path the sourcemap mentions, mapped to the instances built from it. A path can
    /// name more than one instance when a project mounts the same tree twice.
    by_file: HashMap<String, Vec<usize>>,
    relative_path: String,
    stamp: Option<(SystemTime, u64)>,
}

impl Sourcemap {
    /// Reads and indexes the sourcemap named by `lsp.sourcemap`.
    pub fn load(project: &Project, settings: &Settings) -> Result<Self> {
        let Some(relative) = settings.lsp.sourcemap.as_ref() else {
            bail_hint!(
                DISABLED_SOURCEMAP_HINT;
                "lsp.sourcemap is not set, so Biskit has no instance tree for this project"
            );
        };

        let path = project.resolve(relative)?;
        if !path.is_file() {
            bail_hint!(MISSING_SOURCEMAP_HINT; "no sourcemap at {relative}");
        }

        let raw = std::fs::read(&path).with_context(|| format!("failed to read {relative}"))?;
        let stamp = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));

        let root: RawNode = serde_json::from_slice(&raw).with_context(|| {
            format!("failed to parse {relative}; it does not look like a rojo sourcemap")
        })?;

        Ok(Self::index(root, relative.clone(), stamp))
    }

    fn index(root: RawNode, relative_path: String, stamp: Option<(SystemTime, u64)>) -> Self {
        let mut nodes: Vec<Node> = Vec::new();
        let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();

        // An explicit stack rather than recursion: a sourcemap is attacker-shaped input only in
        // the sense that it is generated, but a deeply nested tree should not decide whether the
        // server stays up.
        let mut pending = vec![(root, None::<usize>)];
        while let Some((raw, parent)) = pending.pop() {
            let RawNode {
                name,
                class_name,
                file_paths,
                children,
            } = raw;

            let index = nodes.len();
            let instance_path = match parent {
                None => root_label(&name, &class_name),
                Some(owner) => format!("{}.{name}", nodes[owner].instance_path),
            };

            let file_paths: Vec<String> = file_paths.iter().map(|path| normalize(path)).collect();
            for file in &file_paths {
                by_file.entry(file.clone()).or_default().push(index);
            }

            nodes.push(Node {
                name,
                class_name,
                parent,
                children: Vec::new(),
                file_paths,
                instance_path,
            });
            if let Some(owner) = parent {
                nodes[owner].children.push(index);
            }

            // Reversed, because the stack hands them back in the order they are popped and a
            // caller reading a child list expects the order the sourcemap wrote.
            for child in children.into_iter().rev() {
                pending.push((child, Some(index)));
            }
        }

        Self {
            nodes,
            by_file,
            relative_path,
            stamp,
        }
    }

    /// Builds a tree from sourcemap JSON, for tests that need one without a file on disk.
    #[cfg(test)]
    pub(crate) fn from_json_for_test(raw: &str, relative_path: &str) -> Self {
        let root: RawNode = serde_json::from_str(raw).expect("the test sourcemap parses");
        Self::index(root, relative_path.to_string(), None)
    }

    pub fn reference(&self) -> SourcemapReference {
        let modified = self.stamp.map(|(modified, _)| modified);
        SourcemapReference {
            relative_path: self.relative_path.clone(),
            modified_epoch_seconds: modified.and_then(|time| {
                time.duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|since| since.as_secs())
            }),
            age_seconds: modified.and_then(|time| {
                SystemTime::now()
                    .duration_since(time)
                    .ok()
                    .map(|elapsed| elapsed.as_secs())
            }),
            instances: self.nodes.len(),
        }
    }

    /// Size and modification time of the file this was built from, for cache validity checks.
    pub fn stamp(&self) -> Option<(SystemTime, u64)> {
        self.stamp
    }

    pub fn node(&self, index: usize) -> &Node {
        &self.nodes[index]
    }

    pub fn root(&self) -> usize {
        0
    }

    pub fn child(&self, index: usize, name: &str) -> Option<usize> {
        self.nodes[index]
            .children
            .iter()
            .copied()
            .find(|child| self.nodes[*child].name == name)
    }

    pub fn parent(&self, index: usize) -> Option<usize> {
        self.nodes[index].parent
    }

    /// The first Luau file an instance was built from, which for a script is its source.
    pub fn script_file(&self, index: usize) -> Option<&str> {
        self.nodes[index]
            .file_paths
            .iter()
            .find(|path| is_luau_path(path))
            .map(String::as_str)
    }

    /// The name of the top-level service an instance sits under, which is what decides whether
    /// code is server-only, client-only, or replicated.
    pub fn service_of(&self, index: usize) -> Option<&str> {
        let mut current = index;
        loop {
            let parent = self.nodes[current].parent?;
            if parent == self.root() {
                return Some(&self.nodes[current].name);
            }
            current = parent;
        }
    }

    pub fn describe(&self, index: usize) -> InstanceAnswer {
        let node = self.node(index);
        InstanceAnswer {
            instance_path: node.instance_path.clone(),
            class_name: node.class_name.clone(),
            file_paths: node.file_paths.clone(),
            script_file: self.script_file(index).map(str::to_string),
            service: self.service_of(index).map(str::to_string),
            children: node.children.len(),
        }
    }

    /// Translates in whichever direction the caller asked for.
    pub fn resolve(
        &self,
        instance_path: Option<&str>,
        relative_path: Option<&str>,
    ) -> Result<ResolveAnswer> {
        let instances = match (instance_path, relative_path) {
            (Some(_), Some(_)) => bail_hint!(
                "instance_path translates a DataModel path to files, relative_path translates a \
                 file to its DataModel path";
                "pass either instance_path or relative_path, not both"
            ),
            (Some(path), None) => vec![self.describe(self.resolve_instance_path(path)?)],
            (None, Some(relative)) => {
                let found = self.nodes_for_file(relative);
                if found.is_empty() {
                    bail_hint!(
                        "the sourcemap only names files the rojo project actually syncs; a file \
                         outside the project tree is not in the game at all, and a file added \
                         since the sourcemap was written needs it regenerated";
                        "no instance in the sourcemap is built from {relative}"
                    );
                }
                found.into_iter().map(|node| self.describe(node)).collect()
            }
            (None, None) => bail_hint!(
                "pass instance_path to go from the DataModel to files, or relative_path to go the \
                 other way";
                "neither instance_path nor relative_path was given"
            ),
        };

        Ok(ResolveAnswer {
            instances,
            sourcemap: self.reference(),
        })
    }

    /// Instances built from a project-relative file.
    ///
    /// A file is looked up as written, then as the directory that owns it: rojo names a directory
    /// instance by the directory, and `src/Shared/Combat/init.luau` is the source of the instance
    /// the sourcemap calls `src/Shared/Combat`. Which of the two a given rojo version wrote is not
    /// worth making the caller know.
    pub fn nodes_for_file(&self, relative_path: &str) -> Vec<usize> {
        let normalized = normalize(relative_path);
        if let Some(found) = self.by_file.get(&normalized) {
            return found.clone();
        }

        if is_init_file(&normalized)
            && let Some((directory, _)) = normalized.rsplit_once('/')
            && let Some(found) = self.by_file.get(directory)
        {
            return found.clone();
        }
        Vec::new()
    }

    /// Resolves an instance path written the way Roblox code writes one.
    ///
    /// `game.ReplicatedStorage.Shared.Combat`, `ReplicatedStorage.Shared.Combat`, and
    /// `game:GetService("ReplicatedStorage").Shared.Combat` all name the same instance, and an
    /// agent holding any one of them should not have to rewrite it into some canonical form first.
    pub fn resolve_instance_path(&self, instance_path: &str) -> Result<usize> {
        let segments = parse_instance_path(instance_path);
        if segments.is_empty() {
            bail_hint!(
                "an instance path looks like \"game.ReplicatedStorage.Shared.Combat\"";
                "instance_path names no instance: {instance_path:?}"
            );
        }

        let root = self.root();
        let mut current = root;
        // A path may or may not start at the DataModel, and `game` is the name Roblox code uses
        // for it whatever the rojo project is called.
        let mut rest = segments.as_slice();
        if let Some(first) = segments.first()
            && (first == "game" || *first == self.nodes[root].name)
        {
            rest = &segments[1..];
        }

        for segment in rest {
            match self.child(current, segment) {
                Some(child) => current = child,
                None => {
                    let children: Vec<&str> = self.nodes[current]
                        .children
                        .iter()
                        .take(SUGGESTED_CHILDREN)
                        .map(|child| self.nodes[*child].name.as_str())
                        .collect();
                    let listing = if children.is_empty() {
                        "it has no children".to_string()
                    } else {
                        format!("its children are: {}", children.join(", "))
                    };
                    bail_hint!(
                        format!(
                            "{listing}. Paths are case-sensitive, and an instance added since the \
                             sourcemap was written is not in it."
                        );
                        "no instance named {segment} under {}",
                        self.nodes[current].instance_path
                    );
                }
            }
        }
        Ok(current)
    }
}

/// The DataModel is spelled `game` in every Roblox script, whatever the rojo project named it.
/// A project that is not rooted at a DataModel, such as a model or a package, keeps its own name.
fn root_label(name: &str, class_name: &str) -> String {
    match class_name {
        "DataModel" => "game".to_string(),
        _ => name.to_string(),
    }
}

/// Splits an instance path into names, accepting every spelling Roblox code uses for the same
/// traversal.
pub fn parse_instance_path(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let bytes: Vec<char> = input.chars().collect();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            '.' | '/' | ' ' | '\t' => index += 1,
            ':' => {
                let (method, next) = read_identifier(&bytes, index + 1);
                index = next;
                let (argument, next) = read_call_string(&bytes, index);
                index = next;
                match argument {
                    Some(name) if CHILD_BY_NAME_METHODS.contains(&method.as_str()) => {
                        segments.push(name)
                    }
                    _ => {}
                }
            }
            '[' => {
                let (argument, next) = read_bracket_string(&bytes, index);
                index = next;
                if let Some(name) = argument {
                    segments.push(name);
                }
            }
            _ => {
                let (identifier, next) = read_identifier(&bytes, index);
                // A character that starts no identifier would otherwise spin here forever.
                index = if next == index { index + 1 } else { next };
                if !identifier.is_empty() {
                    segments.push(identifier);
                }
            }
        }
    }
    segments
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

/// The first string literal of a call, as in `:WaitForChild("Combat", 5)`.
fn read_call_string(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from;
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    if index >= chars.len() || chars[index] != '(' {
        return (None, index);
    }

    let mut depth = 0usize;
    let mut literal: Option<String> = None;
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
                    return (literal, index);
                }
            }
            quote @ ('"' | '\'') if literal.is_none() => {
                let (text, next) = read_string(chars, index, quote);
                literal = Some(text);
                index = next;
            }
            _ => index += 1,
        }
    }
    (literal, index)
}

/// The string literal of an index, as in `["Combat"]`.
fn read_bracket_string(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from + 1;
    let mut literal = None;
    while index < chars.len() {
        match chars[index] {
            ']' => return (literal, index + 1),
            quote @ ('"' | '\'') if literal.is_none() => {
                let (text, next) = read_string(chars, index, quote);
                literal = Some(text);
                index = next;
            }
            _ => index += 1,
        }
    }
    (literal, index)
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

/// Sourcemaps written on Windows carry backslashes; every path Biskit reports uses forward
/// slashes, so the two have to meet somewhere.
pub fn normalize(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let trimmed = replaced
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    trimmed.to_string()
}

fn is_luau_path(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|value| value.to_str()),
        Some("luau" | "lua")
    )
}

/// A file rojo folds into the instance its directory names.
pub fn is_init_file(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    matches!(
        name,
        "init.luau"
            | "init.lua"
            | "init.server.luau"
            | "init.server.lua"
            | "init.client.luau"
            | "init.client.lua"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Sourcemap {
        let raw: RawNode = serde_json::from_str(
            r#"{
                "name": "MyGame",
                "className": "DataModel",
                "children": [
                    {
                        "name": "ReplicatedStorage",
                        "className": "ReplicatedStorage",
                        "children": [
                            {
                                "name": "Shared",
                                "className": "Folder",
                                "filePaths": ["src/Shared"],
                                "children": [
                                    {
                                        "name": "Combat",
                                        "className": "ModuleScript",
                                        "filePaths": ["src/Shared/Combat/init.luau"],
                                        "children": [
                                            {
                                                "name": "Damage",
                                                "className": "ModuleScript",
                                                "filePaths": ["src/Shared/Combat/Damage.luau"]
                                            }
                                        ]
                                    }
                                ]
                            }
                        ]
                    },
                    {
                        "name": "ServerScriptService",
                        "className": "ServerScriptService",
                        "children": [
                            {
                                "name": "PlayerService",
                                "className": "ModuleScript",
                                "filePaths": ["src\\Server\\PlayerService.luau"]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();
        Sourcemap::index(raw, "sourcemap.json".to_string(), None)
    }

    #[test]
    fn the_data_model_is_spelled_game_whatever_the_project_is_called() {
        let map = fixture();
        assert_eq!(map.node(map.root()).instance_path, "game");
        let combat = map
            .resolve_instance_path("game.ReplicatedStorage.Shared.Combat")
            .unwrap();
        assert_eq!(
            map.node(combat).instance_path,
            "game.ReplicatedStorage.Shared.Combat"
        );
    }

    #[test]
    fn a_path_resolves_with_or_without_its_root_and_through_get_service() {
        let map = fixture();
        let expected = map
            .resolve_instance_path("game.ReplicatedStorage.Shared.Combat")
            .unwrap();

        for spelling in [
            "ReplicatedStorage.Shared.Combat",
            "MyGame.ReplicatedStorage.Shared.Combat",
            "game:GetService(\"ReplicatedStorage\").Shared.Combat",
            "game.ReplicatedStorage:WaitForChild(\"Shared\").Combat",
            "game.ReplicatedStorage.Shared[\"Combat\"]",
        ] {
            assert_eq!(
                map.resolve_instance_path(spelling).unwrap(),
                expected,
                "failed on {spelling}"
            );
        }
    }

    #[test]
    fn a_missing_segment_names_the_children_it_could_have_been() {
        let map = fixture();
        let error = map
            .resolve_instance_path("game.ReplicatedStorage.Shared.combat")
            .unwrap_err();
        let rendered = crate::errors::render("resolve_instance_path", &error);
        assert!(rendered.contains("no instance named combat"));
        assert!(rendered.contains("its children are: Combat"));
    }

    #[test]
    fn a_file_resolves_back_to_its_instance_whatever_separator_it_was_written_with() {
        let map = fixture();
        let found = map.nodes_for_file("src/Server/PlayerService.luau");
        assert_eq!(found.len(), 1);
        assert_eq!(
            map.node(found[0]).instance_path,
            "game.ServerScriptService.PlayerService"
        );
    }

    /// A rojo version that names a directory instance by the directory leaves the init file
    /// itself unmentioned, and the agent holding the file path should still get an answer.
    #[test]
    fn an_init_file_falls_back_to_the_directory_that_owns_it() {
        let map = fixture();
        let by_init = map.nodes_for_file("src/Shared/init.luau");
        assert_eq!(by_init.len(), 1);
        assert_eq!(
            map.node(by_init[0]).instance_path,
            "game.ReplicatedStorage.Shared"
        );
    }

    #[test]
    fn children_keep_the_order_the_sourcemap_wrote_them_in() {
        let map = fixture();
        let root = map.root();
        let names: Vec<&str> = map
            .node(root)
            .children
            .iter()
            .map(|child| map.node(*child).name.as_str())
            .collect();
        assert_eq!(names, vec!["ReplicatedStorage", "ServerScriptService"]);
    }

    #[test]
    fn the_owning_service_is_the_top_level_ancestor() {
        let map = fixture();
        let damage = map
            .resolve_instance_path("game.ReplicatedStorage.Shared.Combat.Damage")
            .unwrap();
        assert_eq!(map.service_of(damage), Some("ReplicatedStorage"));
        assert_eq!(map.service_of(map.root()), None);
    }

    #[test]
    fn the_script_file_of_an_instance_skips_non_luau_paths() {
        let map = fixture();
        let combat = map
            .resolve_instance_path("game.ReplicatedStorage.Shared.Combat")
            .unwrap();
        assert_eq!(map.script_file(combat), Some("src/Shared/Combat/init.luau"));

        let shared = map
            .resolve_instance_path("game.ReplicatedStorage.Shared")
            .unwrap();
        assert_eq!(map.script_file(shared), None);
    }
}
