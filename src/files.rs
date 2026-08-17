use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::Serialize;

use crate::bail_hint;
use crate::config::Settings;
use crate::errors::hinted;
use crate::lines::LineIndex;
use crate::project::Project;

const LUAU_EXTENSIONS: [&str; 3] = ["luau", "lua", "luaurc"];
const MISSING_DIRECTORY_HINT: &str = "paths are relative to the project root; run list_dir on \".\" \
                                      or on the parent to see what is there";
const REGEX_HINT: &str = "substring_pattern is a Rust regex matched with multi-line and \
                          dot-matches-newline enabled; escape ( ) [ ] . * + ? | \\ to match them \
                          literally";

pub struct FileTools {
    project: Project,
    settings: Settings,
}

#[derive(Debug, Default, Serialize)]
pub struct DirectoryListing {
    /// The listed directory, relative to the project root. Entries below are relative to this,
    /// so the prefix is spelled once rather than once per entry.
    pub base: String,
    pub directories: Vec<String>,
    pub files: Vec<String>,
    /// True when `max_listing_entries` cut the listing short. Omitted when false.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct PatternMatch {
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

#[derive(Debug, Default, Serialize)]
pub struct PatternSearchResult {
    pub matches: BTreeMap<String, Vec<PatternMatch>>,
    /// True when `max_pattern_matches` cut the result set short. Omitted when false.
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct PatternSearchRequest<'a> {
    pub pattern: &'a str,
    pub relative_path: &'a str,
    pub context_lines_before: usize,
    pub context_lines_after: usize,
    pub paths_include_glob: Option<&'a str>,
    pub paths_exclude_glob: Option<&'a str>,
    pub restrict_to_code_files: bool,
    pub max_matches: usize,
}

impl FileTools {
    pub fn new(project: Project, settings: Settings) -> Self {
        Self { project, settings }
    }

    pub fn list_dir(&self, relative_path: &str, recursive: bool) -> Result<DirectoryListing> {
        let base = self.project.resolve(relative_path)?;
        ensure_directory(&base, relative_path)?;

        let mut listing = DirectoryListing {
            base: self.base_label(&base)?,
            ..Default::default()
        };
        let limit = self.settings.tools.max_listing_entries;

        let mut walker = self.walk_builder(&base)?;
        if !recursive {
            walker.max_depth(Some(1));
        }

        for entry in walker.build() {
            let entry = entry?;
            if entry.path() == base {
                continue;
            }
            let relative = relativize_to(entry.path(), &base)?;
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                listing.directories.push(relative);
            } else {
                listing.files.push(relative);
            }
        }

        // Sorting before truncating is what makes a capped listing reproducible. The walker's
        // traversal order is not lexicographic, so cutting the walk short at the cap returned an
        // arbitrary subset that could differ between two calls on an unchanged directory.
        listing.directories.sort();
        listing.files.sort();
        listing.truncated = truncate_listing(&mut listing.directories, &mut listing.files, limit);
        Ok(listing)
    }

    pub fn find_file(&self, file_mask: &str, relative_path: &str) -> Result<Vec<String>> {
        let base = self.project.resolve(relative_path)?;
        ensure_directory(&base, relative_path)?;
        let matcher = compile_glob(file_mask)?;
        let limit = self.settings.tools.max_listing_entries;

        let mut found = Vec::new();
        for entry in self.walk_builder(&base)?.build() {
            let entry = entry?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            // Most files are rejected, so the glob is consulted against borrowed paths and the
            // project-relative string is only built for the ones that survive.
            let Ok(relative) = entry.path().strip_prefix(self.project.root()) else {
                continue;
            };
            if !matcher.is_match(entry.file_name()) && !matcher.is_match(relative) {
                continue;
            }

            found.push(crate::project::normalize_separators(relative));
            if found.len() >= limit {
                break;
            }
        }

        found.sort();
        Ok(found)
    }

    pub fn search_for_pattern(
        &self,
        request: PatternSearchRequest<'_>,
    ) -> Result<PatternSearchResult> {
        let base = self.project.resolve(request.relative_path)?;
        if !base.exists() {
            bail_hint!(
                MISSING_DIRECTORY_HINT;
                "no such file or directory: {}",
                request.relative_path
            );
        }
        let regex = RegexBuilder::new(request.pattern)
            .multi_line(true)
            .dot_matches_new_line(true)
            .build()
            .map_err(|error| {
                hinted(
                    format!("invalid regular expression: {}: {error}", request.pattern),
                    REGEX_HINT,
                )
            })?;

        let include = request.paths_include_glob.map(compile_glob).transpose()?;
        let exclude = request.paths_exclude_glob.map(compile_glob).transpose()?;

        let mut result = PatternSearchResult::default();
        let mut total = 0usize;

        let targets: Vec<_> = if base.is_file() {
            vec![base.clone()]
        } else {
            self.walk_builder(&base)?
                .build()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
                .map(|entry| entry.into_path())
                .collect()
        };

        for path in targets {
            // Ordered cheapest first: the extension test rejects most of a Roblox project by
            // reading a few bytes of the path, so it runs before anything that allocates.
            if request.restrict_to_code_files && !is_code_file(&path) {
                continue;
            }
            let Ok(borrowed) = path.strip_prefix(self.project.root()) else {
                continue;
            };
            if let Some(matcher) = &include
                && !matcher.is_match(borrowed)
            {
                continue;
            }
            if let Some(matcher) = &exclude
                && matcher.is_match(borrowed)
            {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };

            // Most files hold no match at all, so the line structures the snippets need are built
            // on the first hit rather than for every file that was merely read.
            let mut index: Option<LineIndex> = None;
            let mut relative: Option<String> = None;

            for found in regex.find_iter(&contents) {
                if total >= request.max_matches {
                    result.truncated = true;
                    break;
                }
                let index = index.get_or_insert_with(|| LineIndex::new(&contents));
                let relative =
                    relative.get_or_insert_with(|| crate::project::normalize_separators(borrowed));

                let start_line = index.line_of(found.start());
                let end_line = index.line_of(found.end().saturating_sub(1));
                let from = start_line.saturating_sub(request.context_lines_before);
                let to = end_line + request.context_lines_after;

                result
                    .matches
                    .entry(relative.clone())
                    .or_default()
                    .push(PatternMatch {
                        start_line: from + 1,
                        end_line: index.clamp_line(to) + 1,
                        snippet: index.text(from, to).into_owned(),
                    });
                total += 1;
            }

            if result.truncated {
                break;
            }
        }

        Ok(result)
    }

    /// The project-relative label for a listed directory. The root relativizes to the empty
    /// string, which is spelled "." the same way the caller asks for it.
    fn base_label(&self, base: &Path) -> Result<String> {
        let relative = self.project.relativize(base)?;
        if relative.is_empty() {
            return Ok(".".to_string());
        }
        Ok(relative)
    }

    fn walk_builder(&self, base: &Path) -> Result<WalkBuilder> {
        crate::project::walk_builder(base, &self.settings.project)
    }
}

/// Trims a sorted listing to `limit` entries in total, directories first, and reports whether
/// anything was dropped.
fn truncate_listing(directories: &mut Vec<String>, files: &mut Vec<String>, limit: usize) -> bool {
    if directories.len() + files.len() <= limit {
        return false;
    }
    directories.truncate(limit);
    files.truncate(limit - directories.len());
    true
}

fn relativize_to(path: &Path, base: &Path) -> Result<String> {
    let stripped = path
        .strip_prefix(base)
        .with_context(|| format!("path is outside the listed directory: {}", path.display()))?;
    Ok(crate::project::normalize_separators(stripped))
}

fn ensure_directory(base: &Path, relative_path: &str) -> Result<()> {
    if base.is_dir() {
        return Ok(());
    }
    if base.exists() {
        bail_hint!(
            "this path is a file; pass its parent directory, or use search_for_pattern to look \
             inside the file itself";
            "not a directory: {relative_path}"
        );
    }
    bail_hint!(MISSING_DIRECTORY_HINT; "no such directory: {relative_path}");
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher> {
    Ok(Glob::new(pattern)
        .map_err(|error| {
            hinted(
                format!("invalid glob pattern: {pattern}: {error}"),
                "globs use *, ?, [] and **, for example \"*.luau\" or \"src/**/init.luau\"",
            )
        })?
        .compile_matcher())
}

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| LUAU_EXTENSIONS.contains(&extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, FileTools) {
        open_with(Settings::default())
    }

    fn open_with(settings: Settings) -> (tempfile::TempDir, FileTools) {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src").join("Services");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("PlayerService.luau"), "return {}\n").unwrap();
        std::fs::write(dir.path().join("src").join("init.luau"), "return {}\n").unwrap();

        let project = Project::open(dir.path()).unwrap();
        (dir, FileTools::new(project, settings))
    }

    fn search(pattern: &str) -> PatternSearchRequest<'_> {
        PatternSearchRequest {
            pattern,
            relative_path: ".",
            context_lines_before: 0,
            context_lines_after: 0,
            paths_include_glob: None,
            paths_exclude_glob: None,
            restrict_to_code_files: false,
            max_matches: 200,
        }
    }

    #[test]
    fn entries_are_named_relative_to_the_listed_directory() {
        let (_dir, files) = open();
        let listing = files.list_dir("src", true).unwrap();

        assert_eq!(listing.base, "src");
        assert_eq!(listing.directories, vec!["Services".to_string()]);
        assert_eq!(
            listing.files,
            vec![
                "Services/PlayerService.luau".to_string(),
                "init.luau".to_string()
            ]
        );
    }

    #[test]
    fn the_project_root_is_labelled_as_a_dot() {
        let (_dir, files) = open();
        let listing = files.list_dir(".", false).unwrap();

        assert_eq!(listing.base, ".");
        assert_eq!(listing.directories, vec!["src".to_string()]);
    }

    #[test]
    fn a_truncated_listing_is_the_first_entries_by_name() {
        let dir = tempfile::tempdir().unwrap();
        // Written in an order that is not the sorted order, so a walk-order truncation would
        // return a different set from a sorted one.
        for index in [7usize, 3, 9, 1, 5, 0, 8, 2, 6, 4] {
            std::fs::write(
                dir.path().join(format!("Module{index}.luau")),
                "return {}\n",
            )
            .unwrap();
        }

        let mut settings = Settings::default();
        settings.tools.max_listing_entries = 4;
        let files = FileTools::new(Project::open(dir.path()).unwrap(), settings);

        let listing = files.list_dir(".", false).unwrap();
        assert!(listing.truncated);
        assert_eq!(
            listing.files,
            vec![
                "Module0.luau".to_string(),
                "Module1.luau".to_string(),
                "Module2.luau".to_string(),
                "Module3.luau".to_string(),
            ]
        );
        assert_eq!(listing.files, files.list_dir(".", false).unwrap().files);
    }

    #[test]
    fn a_listing_that_fits_is_not_marked_truncated() {
        let (_dir, files) = open();
        let listing = files.list_dir("src", true).unwrap();
        assert!(!listing.truncated);
        assert_eq!(listing.files.len(), 2);
    }

    #[test]
    fn ignored_paths_are_excluded_from_every_walk() {
        let dir = tempfile::tempdir().unwrap();
        let packages = dir.path().join("Packages");
        std::fs::create_dir_all(&packages).unwrap();
        std::fs::write(packages.join("Vendored.luau"), "local Marker = 1\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src").join("Own.luau"),
            "local Marker = 1\n",
        )
        .unwrap();

        let mut settings = Settings::default();
        settings.project.ignored_paths = vec!["Packages/".to_string()];
        let files = FileTools::new(Project::open(dir.path()).unwrap(), settings);

        assert_eq!(
            files.find_file("*.luau", ".").unwrap(),
            vec!["src/Own.luau".to_string()]
        );
        assert_eq!(
            files.list_dir(".", true).unwrap().directories,
            vec!["src".to_string()]
        );

        let found = files.search_for_pattern(search("Marker")).unwrap();
        assert_eq!(
            found.matches.keys().collect::<Vec<_>>(),
            vec!["src/Own.luau"]
        );
    }

    #[test]
    fn an_unignored_project_still_sees_everything() {
        let (_dir, files) = open_with(Settings::default());
        assert_eq!(files.find_file("*.luau", ".").unwrap().len(), 2);
    }

    #[test]
    fn an_invalid_ignored_path_is_reported_rather_than_dropped() {
        let mut settings = Settings::default();
        settings.project.ignored_paths = vec!["[".to_string()];
        let (_dir, files) = open_with(settings);

        let error = files.list_dir(".", false).unwrap_err().to_string();
        assert!(error.contains("ignored_paths"), "unexpected error: {error}");
    }

    #[test]
    fn a_match_reports_the_lines_its_context_window_reaches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Module.luau"),
            "local a = 1\nlocal Target = 2\nlocal c = 3\nlocal d = 4\n",
        )
        .unwrap();
        let files = FileTools::new(Project::open(dir.path()).unwrap(), Settings::default());

        let mut request = search("Target");
        request.context_lines_before = 1;
        request.context_lines_after = 1;
        let found = files.search_for_pattern(request).unwrap();

        let hit = &found.matches["Module.luau"][0];
        assert_eq!((hit.start_line, hit.end_line), (1, 3));
        assert_eq!(hit.snippet, "local a = 1\nlocal Target = 2\nlocal c = 3");
        assert!(!found.truncated);
    }

    #[test]
    fn context_windows_are_clamped_to_the_ends_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Module.luau"), "only Target here\n").unwrap();
        let files = FileTools::new(Project::open(dir.path()).unwrap(), Settings::default());

        let mut request = search("Target");
        request.context_lines_before = 5;
        request.context_lines_after = 5;
        let found = files.search_for_pattern(request).unwrap();

        let hit = &found.matches["Module.luau"][0];
        assert_eq!((hit.start_line, hit.end_line), (1, 1));
        assert_eq!(hit.snippet, "only Target here");
    }

    #[test]
    fn crlf_files_do_not_leak_carriage_returns_into_snippets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Module.luau"),
            "local a = 1\r\nlocal Target = 2\r\nlocal c = 3\r\n",
        )
        .unwrap();
        let files = FileTools::new(Project::open(dir.path()).unwrap(), Settings::default());

        let mut request = search("Target");
        request.context_lines_before = 1;
        request.context_lines_after = 1;
        let found = files.search_for_pattern(request).unwrap();

        let hit = &found.matches["Module.luau"][0];
        assert_eq!(hit.snippet, "local a = 1\nlocal Target = 2\nlocal c = 3");
    }

    #[test]
    fn the_match_cap_sets_the_truncation_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Module.luau"), "Target\n".repeat(10)).unwrap();
        let files = FileTools::new(Project::open(dir.path()).unwrap(), Settings::default());

        let mut request = search("Target");
        request.max_matches = 4;
        let found = files.search_for_pattern(request).unwrap();

        assert!(found.truncated);
        assert_eq!(found.matches["Module.luau"].len(), 4);
    }

    #[test]
    fn a_search_that_matches_nothing_reports_nothing() {
        let (_dir, files) = open();
        let found = files
            .search_for_pattern(search("NotPresentAnywhere"))
            .unwrap();
        assert!(found.matches.is_empty());
        assert!(!found.truncated);
    }

    #[test]
    fn globs_and_the_code_filter_narrow_the_file_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src").join("Kept.luau"), "Target\n").unwrap();
        std::fs::write(dir.path().join("src").join("Skipped.luau"), "Target\n").unwrap();
        std::fs::write(dir.path().join("notes.md"), "Target\n").unwrap();
        let files = FileTools::new(Project::open(dir.path()).unwrap(), Settings::default());

        let matched = |request: PatternSearchRequest<'_>| -> Vec<String> {
            files
                .search_for_pattern(request)
                .unwrap()
                .matches
                .into_keys()
                .collect()
        };

        let mut restricted = search("Target");
        restricted.restrict_to_code_files = true;
        assert_eq!(matched(restricted), ["src/Kept.luau", "src/Skipped.luau"]);

        let mut excluded = search("Target");
        excluded.paths_exclude_glob = Some("**/Skipped.luau");
        assert_eq!(matched(excluded), ["notes.md", "src/Kept.luau"]);

        let mut included = search("Target");
        included.paths_include_glob = Some("src/**");
        assert_eq!(matched(included), ["src/Kept.luau", "src/Skipped.luau"]);
    }
}
