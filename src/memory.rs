use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::{Regex, RegexBuilder};

use crate::bail_hint;
use crate::errors::hinted;
use crate::project::{Project, normalize_separators};

pub const MEMORY_EXTENSION: &str = "md";
const MEM_REFERENCE_PATTERN: &str = r"mem:([A-Za-z0-9._\-/]+)";

const UNKNOWN_MEMORY_HINT: &str = "call list_memories to see which memories exist for this project";
const REGEX_HINT: &str = "the pattern is a Rust regex matched with multi-line and \
                          dot-matches-newline enabled; escape ( ) [ ] . * + ? | \\ to match them \
                          literally";

pub struct MemoryStore {
    project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameOutcome {
    pub from: String,
    pub to: String,
    pub updated_references: Vec<ReferenceUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReferenceUpdate {
    pub memory: String,
    pub occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    pub memory: String,
    pub replacements: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    pub memory: String,
    pub replaced: bool,
}

impl MemoryStore {
    pub fn new(project: Project) -> Self {
        Self { project }
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let root = self.project.memories_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        collect(&root, &root, &mut names)?;
        names.sort();
        Ok(names)
    }

    pub fn read(&self, name: &str) -> Result<String> {
        let path = self.path_for(name)?;
        if !path.is_file() {
            bail_hint!(UNKNOWN_MEMORY_HINT; "memory not found: {}", canonical_name(name));
        }
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read memory: {}", canonical_name(name)))
    }

    pub fn create(&self, name: &str, content: &str, overwrite: bool) -> Result<CreateOutcome> {
        let path = self.path_for(name)?;
        let replaced = path.is_file();
        if replaced && !overwrite {
            bail_hint!(
                "amend it with edit_memory, or pass overwrite: true to replace it wholesale";
                "memory already exists: {}",
                canonical_name(name)
            );
        }
        self.project.bootstrap()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write memory: {}", canonical_name(name)))?;
        Ok(CreateOutcome {
            memory: canonical_name(name),
            replaced,
        })
    }

    pub fn delete(&self, name: &str) -> Result<String> {
        let path = self.path_for(name)?;
        if !path.is_file() {
            bail_hint!(UNKNOWN_MEMORY_HINT; "memory not found: {}", canonical_name(name));
        }
        std::fs::remove_file(&path)?;
        self.prune_empty_dirs(&path)?;
        Ok(canonical_name(name))
    }

    pub fn edit(
        &self,
        name: &str,
        pattern: &str,
        replacement: &str,
        allow_multiple: bool,
    ) -> Result<EditOutcome> {
        let path = self.path_for(name)?;
        if !path.is_file() {
            bail_hint!(UNKNOWN_MEMORY_HINT; "memory not found: {}", canonical_name(name));
        }
        let original = std::fs::read_to_string(&path)?;
        let regex = RegexBuilder::new(pattern)
            .multi_line(true)
            .dot_matches_new_line(true)
            .build()
            .map_err(|error| {
                hinted(
                    format!("invalid regular expression: {pattern}: {error}"),
                    REGEX_HINT,
                )
            })?;

        let matches = regex.find_iter(&original).count();
        if matches == 0 {
            bail_hint!(
                format!(
                    "read_memory {} and copy the target text verbatim into the pattern",
                    canonical_name(name)
                );
                "pattern did not match anything in memory {}: {pattern}",
                canonical_name(name)
            );
        }
        if matches > 1 && !allow_multiple {
            bail_hint!(
                "narrow the pattern with surrounding context, or pass \
                 allow_multiple_occurrences: true to replace every match";
                "pattern matched {matches} times in memory {}",
                canonical_name(name)
            );
        }

        let updated = regex.replace_all(&original, replacement);
        std::fs::write(&path, updated.as_ref())?;
        Ok(EditOutcome {
            memory: canonical_name(name),
            replacements: matches,
        })
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<RenameOutcome> {
        let source = self.path_for(from)?;
        if !source.is_file() {
            bail_hint!(UNKNOWN_MEMORY_HINT; "memory not found: {}", canonical_name(from));
        }
        let target = self.path_for(to)?;
        if target.exists() {
            bail_hint!(
                "pick a different new_name, or delete the existing memory first";
                "memory already exists: {}",
                canonical_name(to)
            );
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&source, &target)?;
        self.prune_empty_dirs(&source)?;

        let updated_references = self.rewrite_references(&stem(from), &stem(to))?;
        Ok(RenameOutcome {
            from: canonical_name(from),
            to: canonical_name(to),
            updated_references,
        })
    }

    fn rewrite_references(&self, from_stem: &str, to_stem: &str) -> Result<Vec<ReferenceUpdate>> {
        let regex = Regex::new(MEM_REFERENCE_PATTERN)?;
        let mut updates = Vec::new();

        for name in self.list()? {
            let path = self.path_for(&name)?;
            let original = std::fs::read_to_string(&path)?;
            let mut occurrences = 0usize;
            let rewritten = regex.replace_all(&original, |captures: &regex::Captures<'_>| {
                let target = &captures[1];
                if reference_matches(target, from_stem) {
                    occurrences += 1;
                    format!("mem:{to_stem}")
                } else {
                    captures[0].to_string()
                }
            });

            if occurrences > 0 {
                std::fs::write(&path, rewritten.as_ref())?;
                updates.push(ReferenceUpdate {
                    memory: name,
                    occurrences,
                });
            }
        }

        updates.sort_by(|a, b| a.memory.cmp(&b.memory));
        Ok(updates)
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        let stem = stem(name);
        if stem.is_empty() {
            bail_hint!(
                "pass a name such as \"style-guide\" or \"architecture/rendering\", without the \
                 .md extension";
                "memory name must not be empty"
            );
        }
        let relative = format!(
            "{}/{}/{}.{MEMORY_EXTENSION}",
            crate::project::BISKIT_DIR,
            crate::project::MEMORIES_DIR,
            stem
        );
        let resolved = self.project.resolve(&relative)?;
        if !resolved.starts_with(self.project.memories_dir()) {
            bail_hint!(
                "memory names are relative and nest with \"/\"; they may not contain \"..\" or \
                 start from a drive or root";
                "memory name escapes the memories directory: {name}"
            );
        }
        Ok(resolved)
    }

    fn prune_empty_dirs(&self, removed: &Path) -> Result<()> {
        let memories_root = self.project.memories_dir();
        let mut cursor = removed.parent().map(Path::to_path_buf);

        while let Some(directory) = cursor {
            if directory == memories_root || !directory.starts_with(&memories_root) {
                break;
            }
            if std::fs::read_dir(&directory)?.next().is_some() {
                break;
            }
            std::fs::remove_dir(&directory)?;
            cursor = directory.parent().map(Path::to_path_buf);
        }
        Ok(())
    }
}

fn reference_matches(reference: &str, from_stem: &str) -> bool {
    stem(reference) == from_stem
}

fn collect(root: &Path, directory: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some(MEMORY_EXTENSION) {
            continue;
        }
        let relative = path.strip_prefix(root)?;
        out.push(strip_extension(&normalize_separators(relative)));
    }
    Ok(())
}

fn strip_extension(name: &str) -> String {
    name.strip_suffix(&format!(".{MEMORY_EXTENSION}"))
        .unwrap_or(name)
        .to_string()
}

fn stem(name: &str) -> String {
    strip_extension(name.trim().trim_matches('/'))
}

fn canonical_name(name: &str) -> String {
    stem(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        project.bootstrap().unwrap();
        (dir, MemoryStore::new(project))
    }

    #[test]
    fn create_bootstraps_an_uninitialised_project() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        let store = MemoryStore::new(project.clone());

        store
            .create("architecture/rendering", "# Rendering", false)
            .unwrap();

        assert!(project.biskit_dir().join(".gitignore").is_file());
        assert!(project.settings_path().is_file());
        assert_eq!(store.list().unwrap(), vec!["architecture/rendering"]);
    }

    #[test]
    fn creates_and_lists_nested_memories() {
        let (_guard, store) = store();
        store
            .create("architecture/rendering", "# Rendering", false)
            .unwrap();
        store.create("style-guide.md", "# Style", false).unwrap();

        assert_eq!(
            store.list().unwrap(),
            vec![
                "architecture/rendering".to_string(),
                "style-guide".to_string()
            ]
        );
        assert_eq!(store.read("architecture/rendering").unwrap(), "# Rendering");
    }

    #[test]
    fn rename_updates_mem_references() {
        let (_guard, store) = store();
        store.create("old-name", "# Old", false).unwrap();
        store
            .create(
                "index",
                "See mem:old-name and mem:old-name.md plus mem:other",
                false,
            )
            .unwrap();

        let outcome = store.rename("old-name", "domain/new-name").unwrap();
        assert_eq!(outcome.updated_references.len(), 1);
        assert_eq!(outcome.updated_references[0].occurrences, 2);
        assert_eq!(
            store.read("index").unwrap(),
            "See mem:domain/new-name and mem:domain/new-name plus mem:other"
        );
    }

    #[test]
    fn edit_rejects_ambiguous_pattern() {
        let (_guard, store) = store();
        store.create("notes", "alpha\nalpha\n", false).unwrap();
        assert!(store.edit("notes", "alpha", "beta", false).is_err());

        let outcome = store.edit("notes", "alpha", "beta", true).unwrap();
        assert_eq!(outcome.replacements, 2);
        assert_eq!(store.read("notes").unwrap(), "beta\nbeta\n");
    }

    #[test]
    fn rejects_path_traversal() {
        let (_guard, store) = store();
        assert!(store.create("../escape", "nope", false).is_err());
    }

    #[test]
    fn delete_prunes_empty_directories() {
        let (_guard, store) = store();
        store.create("nested/deep/leaf", "x", false).unwrap();
        store.delete("nested/deep/leaf").unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn create_refuses_to_clobber_without_overwrite() {
        let (_guard, store) = store();
        let first = store.create("notes", "original", false).unwrap();
        assert!(!first.replaced);

        assert!(store.create("notes", "replacement", false).is_err());
        assert_eq!(store.read("notes").unwrap(), "original");

        let second = store.create("notes", "replacement", true).unwrap();
        assert!(second.replaced);
        assert_eq!(store.read("notes").unwrap(), "replacement");
    }
}
