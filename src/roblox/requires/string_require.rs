use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::project::Project;

const MODULE_SUFFIXES: [&str; 4] = [".luau", ".lua", "/init.luau", "/init.lua"];

pub(super) fn resolve_string_require(
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

pub(super) struct AliasCache {
    root: PathBuf,
    by_directory: HashMap<PathBuf, HashMap<String, PathBuf>>,
}

impl AliasCache {
    pub(super) fn new(project: &Project) -> Self {
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
