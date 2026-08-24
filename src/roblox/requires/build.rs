use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::Result;

use super::super::shared_require::{self, SharedIndex};
use super::super::sourcemap::Sourcemap;
use super::blank_comments;
use super::graph::{Edge, Module, RequireGraph, UnresolvedRequire};
use super::resolve::{resolve, resolve_shared};
use super::scan::{CallKind, find_calls, local_bindings};
use super::string_require::AliasCache;
use crate::config::Settings;
use crate::lines::LineIndex;
use crate::project::{self, Project};

/// What the file set looked like when a graph was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphStamp {
    sourcemap: Option<(SystemTime, u64)>,
    files: usize,
    total_len: u64,
    newest: Option<SystemTime>,
}

/// Rebuilds the graph only when the files it was built from have moved.
pub fn build_or_reuse(
    cached: Option<std::sync::Arc<RequireGraph>>,
    project: &Project,
    settings: &Settings,
    sourcemap: &Sourcemap,
) -> Result<std::sync::Arc<RequireGraph>> {
    let files = luau_files(project, settings, sourcemap)?;
    let stamp = stamp_of(&files, sourcemap);

    if let Some(existing) = cached.filter(|graph| graph.stamp() == &stamp) {
        return Ok(existing);
    }
    Ok(std::sync::Arc::new(build(
        project, settings, sourcemap, files, stamp,
    )?))
}

/// The files the graph is built from: the project walk, plus every Luau file the sourcemap names.
///
/// The walk honours the ignore set, which commonly hides `Packages/`. Those modules are still in
/// the game and are still required by name, so leaving them out reports resolvable requires as
/// dangling. The sourcemap is the authority on what rojo syncs, so it is read alongside the walk.
fn luau_files(
    project: &Project,
    settings: &Settings,
    sourcemap: &Sourcemap,
) -> Result<Vec<PathBuf>> {
    let mut found = std::collections::BTreeSet::new();
    for entry in project::walk_builder(project.root(), project.root(), &settings.project)?
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
            found.insert(path);
        }
    }

    for relative in sourcemap.luau_files() {
        let Ok(path) = project.resolve(relative) else {
            continue;
        };
        if path.is_file() {
            found.insert(path);
        }
    }

    Ok(found.into_iter().collect())
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

#[cfg(test)]
pub(super) mod tests {
    use std::sync::Arc;

    use super::super::Direction;
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

    pub(in crate::roblox::requires) fn fixture() -> (tempfile::TempDir, Project, RequireGraph) {
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
        let files = luau_files(&project, &settings, &sourcemap).unwrap();
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
        let files = luau_files(&project, &settings, &sourcemap).unwrap();
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

    const PACKAGE_SOURCEMAP: &str = r#"{
        "name": "Fixture",
        "className": "DataModel",
        "children": [{
            "name": "ReplicatedStorage",
            "className": "ReplicatedStorage",
            "children": [
                {"name": "Shared", "className": "Folder", "filePaths": ["src/Shared"],
                 "children": [{"name": "Consumer", "className": "ModuleScript",
                               "filePaths": ["src/Shared/Consumer.luau"]}]},
                {"name": "Packages", "className": "Folder", "filePaths": ["Packages"],
                 "children": [{"name": "Signal", "className": "ModuleScript",
                               "filePaths": ["Packages/Signal.luau"]}]}
            ]
        }]
    }"#;

    #[test]
    fn a_module_the_ignore_set_hides_but_the_sourcemap_names_is_still_in_the_graph() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Shared")).unwrap();
        std::fs::create_dir_all(root.join("Packages")).unwrap();
        std::fs::write(root.join(".gitignore"), "Packages/\n").unwrap();
        std::fs::write(root.join("sourcemap.json"), PACKAGE_SOURCEMAP).unwrap();
        std::fs::write(root.join("Packages/Signal.luau"), "return {}\n").unwrap();
        std::fs::write(
            root.join("src/Shared/Consumer.luau"),
            "local Signal = shared(\"Signal\")\nreturn {}\n",
        )
        .unwrap();

        let project = Project::open(root).unwrap();
        let settings = Settings {
            project: crate::config::ProjectSettings {
                shared_require: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let sourcemap = Sourcemap::load(&project, &settings).unwrap();

        let files = luau_files(&project, &settings, &sourcemap).unwrap();
        assert!(
            files.iter().any(|path| path.ends_with("Signal.luau")),
            "a gitignored file rojo syncs into the game is still part of the game: {files:?}"
        );

        let stamp = stamp_of(&files, &sourcemap);
        let graph = build(&project, &settings, &sourcemap, files, stamp).unwrap();

        assert_eq!(
            dependency_paths(&graph, "src/Shared/Consumer.luau"),
            vec!["Packages/Signal.luau".to_string()]
        );
        assert!(
            unresolved_reasons(&graph, "src/Shared/Consumer.luau").is_empty(),
            "a resolvable name must not be reported as naming no module"
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
        let files = luau_files(&project, &settings, &sourcemap).unwrap();
        let stamp = stamp_of(&files, &sourcemap);
        let graph = build(&project, &settings, &sourcemap, files, stamp).unwrap();

        assert_eq!(
            dependency_paths(&graph, "src/Shared/Consumer.luau"),
            vec!["src/Shared/Config.luau".to_string()],
            "only the real require is an edge, and it is not counted twice"
        );
    }
}
