use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::Serialize;

use crate::bail_hint;
use crate::config::Settings;
use crate::errors::hinted;
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

        let mut listing = DirectoryListing::default();
        let limit = self.settings.tools.max_listing_entries;

        let mut walker = self.walk_builder(&base);
        if !recursive {
            walker.max_depth(Some(1));
        }

        for entry in walker.build() {
            let entry = entry?;
            if entry.path() == base {
                continue;
            }
            if listing.directories.len() + listing.files.len() >= limit {
                listing.truncated = true;
                break;
            }
            let relative = self.project.relativize(entry.path())?;
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                listing.directories.push(relative);
            } else {
                listing.files.push(relative);
            }
        }

        listing.directories.sort();
        listing.files.sort();
        Ok(listing)
    }

    pub fn find_file(&self, file_mask: &str, relative_path: &str) -> Result<Vec<String>> {
        let base = self.project.resolve(relative_path)?;
        ensure_directory(&base, relative_path)?;
        let matcher = compile_glob(file_mask)?;
        let limit = self.settings.tools.max_listing_entries;

        let mut found = Vec::new();
        for entry in self.walk_builder(&base).build() {
            let entry = entry?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = self.project.relativize(entry.path())?;
            if matcher.is_match(&name) || matcher.is_match(&relative) {
                found.push(relative);
                if found.len() >= limit {
                    break;
                }
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
            self.walk_builder(&base)
                .build()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
                .map(|entry| entry.into_path())
                .collect()
        };

        for path in targets {
            let relative = self.project.relativize(&path)?;

            if request.restrict_to_code_files && !is_code_file(&path) {
                continue;
            }
            if let Some(matcher) = &include
                && !matcher.is_match(&relative)
            {
                continue;
            }
            if let Some(matcher) = &exclude
                && matcher.is_match(&relative)
            {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = contents.lines().collect();
            let line_starts = line_offsets(&contents);

            for found in regex.find_iter(&contents) {
                if total >= request.max_matches {
                    result.truncated = true;
                    break;
                }
                let start_line = line_index_for(&line_starts, found.start());
                let end_line = line_index_for(&line_starts, found.end().saturating_sub(1));
                let from = start_line.saturating_sub(request.context_lines_before);
                let to =
                    (end_line + request.context_lines_after).min(lines.len().saturating_sub(1));

                result
                    .matches
                    .entry(relative.clone())
                    .or_default()
                    .push(PatternMatch {
                        start_line: from + 1,
                        end_line: to + 1,
                        snippet: lines[from..=to].join("\n"),
                    });
                total += 1;
            }

            if result.truncated {
                break;
            }
        }

        Ok(result)
    }

    fn walk_builder(&self, base: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(base);
        builder
            .hidden(false)
            .git_ignore(self.settings.project.respect_gitignore)
            .git_global(false)
            .git_exclude(self.settings.project.respect_gitignore)
            .follow_links(false)
            .require_git(false);

        for pattern in &self.settings.project.ignored_paths {
            builder.add_ignore(pattern);
        }
        builder.filter_entry(|entry| {
            entry.file_name() != std::ffi::OsStr::new(".git")
                && entry.file_name() != std::ffi::OsStr::new(crate::project::BISKIT_DIR)
        });
        builder
    }
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

fn line_offsets(contents: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (index, byte) in contents.bytes().enumerate() {
        if byte == b'\n' {
            offsets.push(index + 1);
        }
    }
    offsets
}

fn line_index_for(line_starts: &[usize], offset: usize) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}
