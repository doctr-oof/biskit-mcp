use std::collections::HashMap;
use std::path::Path;

use super::super::shared_require::{Resolution, SharedIndex};
use super::super::sourcemap::Sourcemap;
use super::chain::{Step, parse_chain, read_string};
use super::string_require::{AliasCache, resolve_string_require};
use crate::project::Project;

pub(super) fn resolve_shared(
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
pub(super) fn resolve(
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
