use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};

use crate::bail_hint;

pub const BISKIT_DIR: &str = ".biskit";
pub const MEMORIES_DIR: &str = "memories";
pub const SETTINGS_FILE: &str = "settings.yml";
pub const LOCAL_SETTINGS_FILE: &str = "settings.local.yml";

const GITIGNORE_CONTENTS: &str = "settings.local.yml\n";

const RELATIVE_PATH_HINT: &str = "pass a path relative to the project root, such as \
                                  \"src/init.luau\", or \".\" for the root itself";
const ESCAPED_ROOT_HINT: &str = "Biskit only reads inside the project root; drop the leading \
                                 \"..\" segments";

/// Markers consulted only when no ancestor holds a `.biskit` directory.
const FALLBACK_MARKERS: [&str; 2] = [".git", "default.project.json"];

/// Every entry that marks a project root, in the order discovery considers them.
pub const ROOT_MARKERS: [&str; 3] = [BISKIT_DIR, FALLBACK_MARKERS[0], FALLBACK_MARKERS[1]];

#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
}

impl Project {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = canonicalize(root.as_ref())
            .with_context(|| format!("project root not found: {}", root.as_ref().display()))?;
        if !root.is_dir() {
            bail!("project root is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn biskit_dir(&self) -> PathBuf {
        self.root.join(BISKIT_DIR)
    }

    pub fn memories_dir(&self) -> PathBuf {
        self.biskit_dir().join(MEMORIES_DIR)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.biskit_dir().join(SETTINGS_FILE)
    }

    pub fn local_settings_path(&self) -> PathBuf {
        self.biskit_dir().join(LOCAL_SETTINGS_FILE)
    }

    pub fn bootstrap(&self) -> Result<BootstrapReport> {
        let mut report = BootstrapReport::default();
        let biskit = self.biskit_dir();
        report.created_biskit_dir = !biskit.exists();

        std::fs::create_dir_all(self.memories_dir())?;

        let gitignore = biskit.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, GITIGNORE_CONTENTS)?;
            report.created_gitignore = true;
        }

        let settings = self.settings_path();
        if !settings.exists() {
            std::fs::write(&settings, crate::config::DEFAULT_SETTINGS_YML)?;
            report.created_settings = true;
        }

        let local = self.local_settings_path();
        if !local.exists() {
            std::fs::write(&local, crate::config::DEFAULT_LOCAL_SETTINGS_YML)?;
            report.created_local_settings = true;
        }

        Ok(report)
    }

    /// Resolves a project-relative path, refusing traversal outside the project root.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let candidate = Path::new(relative);
        if candidate.is_absolute() {
            bail_hint!(RELATIVE_PATH_HINT; "path must be relative to the project root: {relative}");
        }

        let mut resolved = self.root.clone();
        for component in candidate.components() {
            match component {
                Component::Normal(part) => resolved.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !resolved.pop() || !resolved.starts_with(&self.root) {
                        bail_hint!(ESCAPED_ROOT_HINT; "path escapes the project root: {relative}");
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    bail_hint!(
                        RELATIVE_PATH_HINT;
                        "path must be relative to the project root: {relative}"
                    )
                }
            }
        }

        if !resolved.starts_with(&self.root) {
            bail_hint!(ESCAPED_ROOT_HINT; "path escapes the project root: {relative}");
        }
        Ok(resolved)
    }

    pub fn relativize(&self, absolute: &Path) -> Result<String> {
        let stripped = absolute
            .strip_prefix(&self.root)
            .with_context(|| format!("path is outside the project: {}", absolute.display()))?;
        Ok(normalize_separators(stripped))
    }
}

/// The one walker every project traversal is built from.
///
/// Both the file tools and the Luau file scan need the same exclusions, and when they were
/// configured separately they drifted: the scan descended into `.git`, which on a real repository
/// is tens of thousands of stat calls that can never yield a `.luau` file.
///
/// `ignore` detects `.git` only so it can locate gitignore files; it never excludes the directory
/// from traversal on its own, so the exclusion has to be stated here.
pub fn walk_builder(base: &Path, settings: &crate::config::ProjectSettings) -> Result<WalkBuilder> {
    let mut builder = WalkBuilder::new(base);
    builder
        .hidden(false)
        .git_ignore(settings.respect_gitignore)
        .git_exclude(settings.respect_gitignore)
        .git_global(false)
        .require_git(false)
        .follow_links(false);

    if !settings.ignored_paths.is_empty() {
        builder.overrides(build_overrides(base, &settings.ignored_paths)?);
    }

    builder.filter_entry(|entry| {
        let name = entry.file_name();
        name != OsStr::new(".git") && name != OsStr::new(BISKIT_DIR)
    });
    Ok(builder)
}

/// Turns `project.ignored_paths` into exclusions.
///
/// `WalkBuilder::add_ignore` takes the path of an ignore *file*, not a pattern, so passing the
/// patterns to it excluded nothing at all. An override glob prefixed with `!` is the API that
/// carries gitignore syntax, which is what the setting has always been documented as taking.
fn build_overrides(base: &Path, patterns: &[String]) -> Result<Override> {
    let mut overrides = OverrideBuilder::new(base);
    for pattern in patterns {
        let negated = match pattern.strip_prefix('!') {
            // A leading "!" in gitignore syntax re-includes, which for a list named
            // "ignored_paths" would invert the caller's stated intent. Take it literally instead.
            Some(rest) => rest,
            None => pattern.as_str(),
        };
        overrides.add(&format!("!{negated}")).map_err(|error| {
            crate::errors::hinted(
                format!("invalid project.ignored_paths entry {pattern:?}: {error}"),
                "entries use gitignore syntax, one pattern per entry, for example \"Packages/\" \
                 or \"**/node_modules\"",
            )
        })?;
    }
    overrides
        .build()
        .context("failed to compile project.ignored_paths")
}

/// `std::fs::canonicalize` yields Windows verbatim paths, which many tools mishandle.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    let Some(text) = resolved.to_str() else {
        return Ok(resolved);
    };
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        return Ok(PathBuf::from(format!(r"\\{stripped}")));
    }
    if let Some(stripped) = text.strip_prefix(r"\\?\")
        && stripped.as_bytes().get(1) == Some(&b':')
    {
        return Ok(PathBuf::from(stripped));
    }
    Ok(resolved)
}

/// Walks up from `start` and returns the ancestor that owns the project.
///
/// Agents launch MCP servers with a working directory that is usually, but not always, the project
/// root, so the ascent lets a nested working directory still resolve to the right project.
///
/// A `.biskit` directory anywhere in the chain wins over a nearer `.git` or `default.project.json`,
/// because it is the only marker that states the directory is deliberately a Biskit project.
pub fn discover_root(start: &Path) -> Option<PathBuf> {
    let start = canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let nearest = |markers: &[&str]| {
        start
            .ancestors()
            .find(|ancestor| markers.iter().any(|marker| ancestor.join(marker).exists()))
            .map(Path::to_path_buf)
    };
    nearest(&[BISKIT_DIR]).or_else(|| nearest(&FALLBACK_MARKERS))
}

pub fn normalize_separators(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BootstrapReport {
    pub created_biskit_dir: bool,
    pub created_gitignore: bool,
    pub created_settings: bool,
    pub created_local_settings: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_finds_the_marked_ancestor_from_a_nested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let nested = root.join("src").join("client");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(root.join(BISKIT_DIR)).unwrap();

        let found = discover_root(&nested).unwrap();
        assert_eq!(found, canonicalize(&root).unwrap());
    }

    #[test]
    fn discovery_prefers_the_nearest_marked_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("workspace");
        let inner = outer.join("packages").join("game");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::create_dir_all(inner.join(BISKIT_DIR)).unwrap();

        let found = discover_root(&inner).unwrap();
        assert_eq!(found, canonicalize(&inner).unwrap());
    }

    #[test]
    fn discovery_prefers_an_outer_biskit_dir_over_a_nearer_fallback_marker() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("workspace");
        let inner = outer.join("packages").join("game");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(outer.join(BISKIT_DIR)).unwrap();
        std::fs::write(inner.join("default.project.json"), "{}").unwrap();

        let found = discover_root(&inner).unwrap();
        assert_eq!(found, canonicalize(&outer).unwrap());
    }

    #[test]
    fn discovery_prefers_the_nearest_fallback_marker_when_no_biskit_dir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("workspace");
        let inner = outer.join("packages").join("game");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::write(inner.join("default.project.json"), "{}").unwrap();

        let found = discover_root(&inner).unwrap();
        assert_eq!(found, canonicalize(&inner).unwrap());
    }

    #[test]
    fn discovery_accepts_a_rojo_project_file_as_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("default.project.json"), "{}").unwrap();

        let found = discover_root(&root).unwrap();
        assert_eq!(found, canonicalize(&root).unwrap());
    }

    #[test]
    fn resolve_refuses_to_escape_the_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert!(project.resolve("../outside.luau").is_err());
        assert!(project.resolve("src/../src/init.luau").is_ok());
    }
}
