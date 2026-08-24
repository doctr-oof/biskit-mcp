//! Resolution for the Carpenter fork's `shared("Name")` string require.

use std::collections::HashMap;

use super::sourcemap::Sourcemap;

/// The global treated as a string require.
pub const GLOBAL_NAME: &str = "shared";

/// What a `shared("Name")` lookup came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The project-relative path of the module the call resolves to.
    Found(String),
    NotFound,
    /// Every candidate that tied for nearest, sorted.
    Ambiguous(Vec<String>),
}

struct Entry {
    relative_path: String,
    key_path: String,
}

/// Case-insensitive index of the project's Luau files, keyed by file stem.
pub struct SharedIndex {
    entries: HashMap<String, Vec<Entry>>,
    module_names: HashMap<String, String>,
}

impl SharedIndex {
    pub fn build(relative_paths: &[String], sourcemap: &Sourcemap) -> Self {
        let mut entries: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut module_names = HashMap::with_capacity(relative_paths.len());

        for relative_path in relative_paths {
            module_names.insert(relative_path.clone(), module_name(sourcemap, relative_path));

            let Some((key, key_path)) = index_key_for(relative_path) else {
                continue;
            };
            entries.entry(key).or_default().push(Entry {
                relative_path: relative_path.clone(),
                key_path,
            });
        }

        Self {
            entries,
            module_names,
        }
    }

    /// How many files carry a stem the index can be asked about.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `name` is a bare stem (`"Foo"`) or a partial path (`"jobs/Foo"`), case-insensitive.
    pub fn resolve(&self, name: &str, requiring_relative_path: &str) -> Resolution {
        if name.is_empty() {
            return Resolution::NotFound;
        }

        let mut needle = name.to_ascii_lowercase().replace('\\', "/");
        for extension in [".luau", ".lua"] {
            if needle.len() > extension.len() && needle.ends_with(extension) {
                needle.truncate(needle.len() - extension.len());
                break;
            }
        }
        if needle.is_empty() {
            return Resolution::NotFound;
        }

        let qualified = needle.contains('/');
        let Some(bucket) = self.entries.get(last_segment(&needle)) else {
            return Resolution::NotFound;
        };

        let matches: Vec<&Entry> = bucket
            .iter()
            .filter(|entry| !qualified || ends_with_path_suffix(&entry.key_path, &needle))
            .filter(|entry| entry.relative_path != requiring_relative_path)
            .collect();

        match matches.as_slice() {
            [] => return Resolution::NotFound,
            [only] => return Resolution::Found(only.relative_path.clone()),
            _ => {}
        }

        let requiring = self
            .module_names
            .get(requiring_relative_path)
            .map_or(requiring_relative_path, String::as_str);

        let mut best_score = 0;
        let mut best: Vec<&Entry> = Vec::new();
        for entry in &matches {
            let module = self
                .module_names
                .get(&entry.relative_path)
                .map_or(entry.relative_path.as_str(), String::as_str);
            let score = common_segment_count(module, requiring);
            if score > best_score {
                best_score = score;
                best.clear();
            }
            if score == best_score {
                best.push(entry);
            }
        }

        if let [only] = best.as_slice() {
            return Resolution::Found(only.relative_path.clone());
        }

        let mut candidates: Vec<String> = matches
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect();
        candidates.sort();
        Resolution::Ambiguous(candidates)
    }
}

fn module_name(sourcemap: &Sourcemap, relative_path: &str) -> String {
    let Some(node) = sourcemap.nodes_for_file(relative_path).first().copied() else {
        return relative_path.to_string();
    };

    let mut segments = Vec::new();
    let mut current = Some(node);
    while let Some(index) = current {
        let node = sourcemap.node(index);
        segments.push(match node.parent {
            Some(_) => node.name.as_str(),
            None => node.instance_path.as_str(),
        });
        current = node.parent;
    }
    segments.reverse();
    segments.join("/")
}

fn index_key_for(relative_path: &str) -> Option<(String, String)> {
    let without_extension = strip_luau_extension(relative_path)?;
    let mut key_path = without_extension;
    let mut stem = last_segment(key_path);

    if stem.eq_ignore_ascii_case("init") {
        key_path = match without_extension.rfind('/') {
            Some(cut) => &without_extension[..cut],
            None => "",
        };
        stem = last_segment(key_path);
    }

    if stem.is_empty() {
        return None;
    }
    Some((stem.to_ascii_lowercase(), key_path.to_ascii_lowercase()))
}

fn strip_luau_extension(path: &str) -> Option<&str> {
    path.strip_suffix(".luau")
        .or_else(|| path.strip_suffix(".lua"))
}

fn last_segment(path: &str) -> &str {
    match path.rfind('/') {
        Some(cut) => &path[cut + 1..],
        None => path,
    }
}

fn ends_with_path_suffix(path: &str, suffix: &str) -> bool {
    if path == suffix {
        return true;
    }
    match path.len().checked_sub(suffix.len()) {
        Some(cut) if cut > 0 => path.as_bytes()[cut - 1] == b'/' && &path[cut..] == suffix,
        _ => false,
    }
}

fn common_segment_count(left: &str, right: &str) -> usize {
    let is_separator = |byte: u8| byte == b'/' || byte == b'\\';
    let mut count = 0;

    let mut left_rest = Some(left);
    let mut right_rest = Some(right);
    while let (Some(left_here), Some(right_here)) = (left_rest, right_rest) {
        let left_end = left_here.bytes().position(is_separator);
        let right_end = right_here.bytes().position(is_separator);

        let left_segment = left_end.map_or(left_here, |cut| &left_here[..cut]);
        let right_segment = right_end.map_or(right_here, |cut| &right_here[..cut]);
        if left_segment != right_segment {
            break;
        }
        count += 1;

        left_rest = left_end.map(|cut| &left_here[cut + 1..]);
        right_rest = right_end.map(|cut| &right_here[cut + 1..]);
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sourcemap() -> Sourcemap {
        Sourcemap::from_json_for_test(
            r#"{
                "name": "MyGame",
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
                                {"name": "Config", "className": "ModuleScript",
                                 "filePaths": ["src/Shared/Config.luau"]},
                                {"name": "Consumer", "className": "ModuleScript",
                                 "filePaths": ["src/Shared/Consumer.luau"]},
                                {"name": "Jobs", "className": "Folder",
                                 "filePaths": ["src/Shared/Jobs"],
                                 "children": [{"name": "Runner", "className": "ModuleScript",
                                               "filePaths": ["src/Shared/Jobs/Runner.luau"]}]}
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
                                 "filePaths": ["src/Server/Config.luau"]},
                                {"name": "Caller", "className": "ModuleScript",
                                 "filePaths": ["src/Server/Caller.luau"]}
                            ]
                        }]
                    }
                ]
            }"#,
            "sourcemap.json",
        )
    }

    fn index(paths: &[&str]) -> SharedIndex {
        let owned: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        SharedIndex::build(&owned, &sourcemap())
    }

    #[test]
    fn a_bare_stem_resolves_to_the_only_file_carrying_it() {
        let index = index(&["src/Shared/Consumer.luau", "src/Shared/Jobs/Runner.luau"]);
        assert_eq!(
            index.resolve("Runner", "src/Shared/Consumer.luau"),
            Resolution::Found("src/Shared/Jobs/Runner.luau".to_string())
        );
    }

    #[test]
    fn the_lookup_is_case_insensitive_in_both_directions() {
        let index = index(&["src/Shared/Consumer.luau", "src/Shared/Jobs/Runner.luau"]);
        for spelling in ["runner", "RUNNER", "rUnNeR"] {
            assert_eq!(
                index.resolve(spelling, "src/Shared/Consumer.luau"),
                Resolution::Found("src/Shared/Jobs/Runner.luau".to_string()),
                "{spelling} did not resolve"
            );
        }
    }

    #[test]
    fn a_written_out_extension_names_the_same_module() {
        let index = index(&["src/Shared/Consumer.luau", "src/Shared/Jobs/Runner.luau"]);
        for spelling in ["Runner.luau", "Runner.lua", "Jobs/Runner.luau"] {
            assert_eq!(
                index.resolve(spelling, "src/Shared/Consumer.luau"),
                Resolution::Found("src/Shared/Jobs/Runner.luau".to_string()),
                "{spelling} did not resolve"
            );
        }
    }

    #[test]
    fn a_partial_path_narrows_to_the_matching_directory() {
        let index = index(&[
            "src/Shared/Config.luau",
            "src/Server/Config.luau",
            "src/Shared/Consumer.luau",
        ]);
        assert_eq!(
            index.resolve("Server/Config", "src/Shared/Consumer.luau"),
            Resolution::Found("src/Server/Config.luau".to_string())
        );
        assert_eq!(
            index.resolve("shared/config", "src/Server/Caller.luau"),
            Resolution::Found("src/Shared/Config.luau".to_string())
        );
    }

    #[test]
    fn a_partial_path_only_matches_on_a_segment_boundary() {
        let index = index(&["src/Shared/Config.luau", "src/Shared/Consumer.luau"]);
        assert_eq!(
            index.resolve("ared/Config", "src/Shared/Consumer.luau"),
            Resolution::NotFound
        );
    }

    #[test]
    fn a_backslash_in_the_needle_is_a_path_separator() {
        let index = index(&["src/Shared/Jobs/Runner.luau", "src/Shared/Consumer.luau"]);
        assert_eq!(
            index.resolve("Jobs\\Runner", "src/Shared/Consumer.luau"),
            Resolution::Found("src/Shared/Jobs/Runner.luau".to_string())
        );
    }

    #[test]
    fn an_init_file_is_indexed_under_its_directory() {
        let index = index(&["src/Shared/Jobs/init.luau", "src/Shared/Consumer.luau"]);
        assert_eq!(
            index.resolve("Jobs", "src/Shared/Consumer.luau"),
            Resolution::Found("src/Shared/Jobs/init.luau".to_string())
        );
        assert_eq!(
            index.resolve("init", "src/Shared/Consumer.luau"),
            Resolution::NotFound,
            "the folded file is not addressable as \"init\""
        );
    }

    #[test]
    fn the_nearest_candidate_in_the_instance_tree_wins() {
        let index = index(&[
            "src/Shared/Config.luau",
            "src/Server/Config.luau",
            "src/Shared/Consumer.luau",
            "src/Server/Caller.luau",
        ]);
        assert_eq!(
            index.resolve("Config", "src/Shared/Consumer.luau"),
            Resolution::Found("src/Shared/Config.luau".to_string())
        );
        assert_eq!(
            index.resolve("Config", "src/Server/Caller.luau"),
            Resolution::Found("src/Server/Config.luau".to_string())
        );
    }

    #[test]
    fn proximity_falls_back_to_directory_depth_without_a_sourcemap() {
        let owned: Vec<String> = ["a/deep/Config.luau", "b/Config.luau", "a/deep/Caller.luau"]
            .iter()
            .map(|path| path.to_string())
            .collect();
        let empty = Sourcemap::from_json_for_test(
            r#"{"name": "MyGame", "className": "DataModel"}"#,
            "sourcemap.json",
        );
        let index = SharedIndex::build(&owned, &empty);

        assert_eq!(
            index.resolve("Config", "a/deep/Caller.luau"),
            Resolution::Found("a/deep/Config.luau".to_string())
        );
    }

    #[test]
    fn a_genuine_tie_is_reported_rather_than_guessed_at() {
        let index = index(&["src/Shared/Config.luau", "src/Server/Config.luau"]);
        assert_eq!(
            index.resolve("Config", "elsewhere/Unmapped.luau"),
            Resolution::Ambiguous(vec![
                "src/Server/Config.luau".to_string(),
                "src/Shared/Config.luau".to_string(),
            ])
        );
    }

    #[test]
    fn a_module_never_resolves_to_itself() {
        let index = index(&["src/Shared/Config.luau"]);
        assert_eq!(
            index.resolve("Config", "src/Shared/Config.luau"),
            Resolution::NotFound
        );
    }

    #[test]
    fn a_name_no_file_carries_is_not_found() {
        let index = index(&["src/Shared/Config.luau"]);
        assert_eq!(
            index.resolve("Nonexistent", "src/Shared/Config.luau"),
            Resolution::NotFound
        );
        assert_eq!(
            index.resolve("", "src/Shared/Config.luau"),
            Resolution::NotFound
        );
        assert_eq!(
            index.resolve(".luau", "src/Shared/Config.luau"),
            Resolution::NotFound
        );
    }

    #[test]
    fn only_luau_files_are_indexed() {
        let index = index(&["src/Shared/Config.json", "src/Shared/Notes.md"]);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn segments_are_counted_only_while_they_match() {
        assert_eq!(common_segment_count("game/A/B", "game/A/B"), 3);
        assert_eq!(common_segment_count("game/A/B", "game/A/C"), 2);
        assert_eq!(common_segment_count("game/A", "game/A/B/C"), 2);
        assert_eq!(common_segment_count("other/A", "game/A"), 0);
        assert_eq!(common_segment_count("game\\A\\B", "game/A/C"), 2);
    }
}
