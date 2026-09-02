Searches file contents with a Rust regular expression, reporting snippets, matching paths, or per-file counts.

## Parameters
- `substring_pattern` (string, required) — Rust regex matched against file contents.
- `relative_path` (string, optional, default `"."`) — directory **or file** to search, relative to the project root.
- `context_lines_before` (usize, optional, default 0).
- `context_lines_after` (usize, optional, default 0).
- `paths_include_glob` (string, optional) — keep only paths matching this glob.
- `paths_exclude_glob` (string, optional) — drop paths matching this glob; applied after the include glob, so it wins.
- `restrict_search_to_code_files` (bool, optional, default false) — only `.luau`, `.lua`, `.luaurc` (`LUAU_EXTENSIONS`, `src/files.rs:16`).
- `mode` (enum, optional, default `"snippets"`) — `"snippets"`, `"files"`, `"counts"` (`SearchOutputMode`, `src/server/requests.rs:94`).
- `case_insensitive` (bool, optional, default false).
- `dot_matches_newline` (bool, optional, default false).
- No `max_matches` parameter: the cap comes from `tools.max_pattern_matches` and is injected by the handler.

## Implementation
- Handler: `src/server/mod.rs:326` (`async fn search_for_pattern`), request type `SearchForPatternRequest` in `src/server/requests.rs:60`, description text in `src/server/descriptions.rs:35`.
- Builds a `PatternSearchRequest` and calls `FileTools::search_for_pattern` at `src/files.rs:166`, passing `max_matches: settings.tools.max_pattern_matches` (default 200).
- Regex is built with `multi_line(true)` always, plus `dot_matches_new_line` and `case_insensitive` from the request. `^`/`$` therefore bind to line ends.
- Path resolution uses `Project::resolve`, then only an existence check — unlike `list_dir` and `find_file` a file path is accepted and searched directly (`base.is_file()` yields a single-element target list).
- Directory targets are collected from the shared `crate::project::walk_builder`, so gitignore (`project.respect_gitignore`), `project.ignored_paths` (anchored at the project root), `.git` and `.biskit` filtering all behave as in `mem:Tools/list_dir`.
- Per file: code-file filter, then include glob, then exclude glob, both matched against the path relative to the project root. `std::fs::read_to_string` failures (binary, unreadable) silently skip the file.
- Snippet mode uses `LineIndex` (`src/lines.rs`) built lazily per file: `line_of` gives the 0-based start line and the line of `end - 1`; the window is `start - context_lines_before` (saturating) to `end + context_lines_after`, reported 1-based with `end_line` clamped to the last line of the file. `LineIndex::text` folds CRLF to LF and strips the trailing terminator, so snippets never carry `\r`.
- Result `PatternSearchResult` (`src/files.rs:48`): `matches` (map path -> `{start_line, end_line, snippet}`) and `total_matches` in snippets mode; `files` (array) in files mode; `counts` (map path -> count) plus `total_matches` in counts mode; `truncated` and `note` when cut. Unused fields are omitted from the JSON.

## Edge cases
- The cap means different things per mode: in snippets mode `max_pattern_matches` counts **matches** across all files; in files and counts modes it counts **files**. In snippets mode the cut can land inside a file, so the last file listed may be incomplete and the true total is unknown — the `note` says exactly that and suggests `mode: "counts"`, a narrower `relative_path`, or raising `tools.max_pattern_matches` in `.biskit/settings.yml`.
- A count reported in counts mode is that file's real full count; only the number of *files* is capped, so `total_matches` there is the sum over reported files only.
- `dot_matches_newline` makes a plain `.*` run to the end of the file and return the whole file as one match; off by default for that reason.
- Naming an ignored directory as `relative_path` searches it anyway; an ignored directory below the base is still pruned.
- Errors: `no such file or directory: <path>` (hint: paths are relative to the project root); `invalid regular expression: <pattern>: <error>` with the `REGEX_HINT` about multi-line mode and escaping; `invalid glob pattern` for either glob; root-escape errors from `Project::resolve`.
- Snippet results are the most likely to blow `tools.max_answer_chars` (default 50000) — `Biskit::ok` (`src/server/mod.rs:60`) rejects the whole call rather than truncating, with the `OVERRUN_HINT` from `src/server/results.rs:11`.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/server/results.rs`
- `src/files.rs`
- `src/lines.rs`
- `src/project.rs`
- `src/config.rs`
- `src/errors.rs`

Siblings: `mem:Tools/list_dir`, `mem:Tools/find_file`.
