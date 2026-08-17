//! Timing harness for the paths the performance audit identified as hot.
//!
//! Run with `cargo bench`. There is no third-party bench framework: each case is a closure timed
//! over a sample budget and reported as a median, so two runs are directly comparable.
//! `BISKIT_BENCH_FILTER` selects a subset of cases by substring.
//!
//! Cases come in pairs. The `before` case re-implements the shape the code had prior to the
//! optimisation, inline and deliberately unshared; the `after` case calls the real code. Running
//! both in one process on one machine is what makes the ratio between them meaningful.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use biskit_mcp::config::Settings;
use biskit_mcp::files::{FileTools, PatternSearchRequest};
use biskit_mcp::lines::LineIndex;
use biskit_mcp::lsp::name_path::NamePathPattern;
use biskit_mcp::lsp::protocol::{DocumentSymbol, DocumentSymbolResponse, Position, Range};
use biskit_mcp::lsp::queries::prefilter_by_literal;
use biskit_mcp::lsp::session::LanguageServerHandle;
use biskit_mcp::lsp::symbols::{SymbolNode, build_tree, disambiguate};
use biskit_mcp::lsp::uri;
use biskit_mcp::project::Project;

/// Files in the generated fixture project. Large enough that the walk and the search are
/// dominated by real work rather than by setup.
const FIXTURE_FILES: usize = 600;
/// Loose objects in the fixture `.git`, standing in for the object store of a real repository.
const FIXTURE_GIT_OBJECTS: usize = 2_400;
const SYMBOLS_PER_FILE: usize = 40;

fn main() {
    let filter = std::env::var("BISKIT_BENCH_FILTER").unwrap_or_default();
    let mut reporter = Reporter::new(filter);
    let fixture = Fixture::build();

    bench_name_path_matching(&mut reporter);
    bench_symbol_tree_build(&mut reporter);
    bench_uri_from_path(&mut reporter);
    bench_line_slicing(&mut reporter);
    bench_literal_prefilter(&mut reporter, &fixture);
    bench_project_walk(&mut reporter, &fixture);
    bench_pattern_search(&mut reporter, &fixture);
    bench_real_project(&mut reporter);

    reporter.finish();
}

/// The generated fixture is uniform in a way no real codebase is: same file size, same symbol
/// density, no vendored packages, no sourcemap. Pointing `BISKIT_BENCH_PROJECT` at a checkout
/// measures the same paths against a tree that was not built to flatter them.
fn bench_real_project(reporter: &mut Reporter) {
    let Ok(root) = std::env::var("BISKIT_BENCH_PROJECT") else {
        reporter.note(
            "set BISKIT_BENCH_PROJECT to a checkout to measure the walk against a real tree"
                .to_string(),
        );
        return;
    };
    let Ok(project) = Project::open(&root) else {
        reporter.note(format!(
            "BISKIT_BENCH_PROJECT is not a readable directory: {root}"
        ));
        return;
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let handle = LanguageServerHandle::new(project.clone(), Settings::default());
    let tools = FileTools::new(project.clone(), Settings::default());

    let files = runtime.block_on(handle.resolve_luau_files(None)).unwrap();
    let bytes: usize = files
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len() as usize)
        .sum();
    reporter.note(format!(
        "real project {root}: {} Luau files, {} KiB",
        files.len(),
        bytes / 1024
    ));

    reporter.case("REAL walk project for .luau", files.len(), || {
        black_box(
            runtime
                .block_on(handle.resolve_luau_files(None))
                .unwrap()
                .len(),
        );
    });

    let absent = "NoSuchSymbolExistsAnywhereHere";
    let kept = runtime
        .block_on(prefilter_by_literal(files.clone(), Some(absent)))
        .unwrap()
        .len();
    reporter.note(format!(
        "REAL prefilter keeps {kept}/{} files for a name the project does not define",
        files.len()
    ));

    reporter.case("REAL prefilter, name absent", files.len(), || {
        black_box(
            runtime
                .block_on(prefilter_by_literal(files.clone(), Some(absent)))
                .unwrap()
                .len(),
        );
    });

    reporter.case("REAL search_for_pattern, no match", files.len(), || {
        let found = tools
            .search_for_pattern(PatternSearchRequest {
                pattern: absent,
                relative_path: ".",
                context_lines_before: 2,
                context_lines_after: 2,
                paths_include_glob: None,
                paths_exclude_glob: None,
                restrict_to_code_files: true,
                max_matches: 200,
            })
            .unwrap();
        black_box(found.matches.len());
    });
}

// ---------------------------------------------------------------------------------------------
// E1: name path matching
// ---------------------------------------------------------------------------------------------

/// `collect_matches` runs this once per symbol node in every scanned file, so one comparison is
/// multiplied by the whole project's symbol count.
fn bench_name_path_matching(reporter: &mut Reporter) {
    let nodes = flat_symbols(SYMBOLS_PER_FILE);
    let miss = NamePathPattern::parse("GetPlayerMaid", false);
    let hit = NamePathPattern::parse("Service12/method3", false);

    for (label, pattern) in [("miss", &miss), ("hit", &hit)] {
        reporter.case(
            &format!("E1 name path match, {label} [before]"),
            nodes.len(),
            || {
                let mut found = 0usize;
                for node in &nodes {
                    // The pre-optimisation shape: split the stored name path into owned segments,
                    // then compare the segment slice.
                    let ancestors: Vec<String> =
                        node.name_path.split('/').map(str::to_string).collect();
                    if naive_matches(pattern, &ancestors) {
                        found += 1;
                    }
                }
                black_box(found);
            },
        );

        reporter.case(
            &format!("E1 name path match, {label} [after]"),
            nodes.len(),
            || {
                let mut found = 0usize;
                for node in &nodes {
                    if pattern.matches(&node.name_path) {
                        found += 1;
                    }
                }
                black_box(found);
            },
        );
    }
}

/// Mirrors the allocating comparison `NamePathPattern::matches` used to perform.
fn naive_matches(pattern: &NamePathPattern, candidate: &[String]) -> bool {
    let segments = pattern.segments();
    if segments.is_empty() || candidate.is_empty() || segments.len() > candidate.len() {
        return false;
    }
    let offset = candidate.len() - segments.len();
    if pattern.is_absolute() && offset != 0 {
        return false;
    }
    let tail = &candidate[offset..];
    segments
        .iter()
        .enumerate()
        .all(|(index, expected)| tail[index].as_str() == expected.as_str())
}

// ---------------------------------------------------------------------------------------------
// E2: symbol tree construction
// ---------------------------------------------------------------------------------------------

/// Every `documentSymbol` response is converted through `disambiguate`, once per sibling group at
/// every level of the tree.
fn bench_symbol_tree_build(reporter: &mut Reporter) {
    let response = document_symbols(SYMBOLS_PER_FILE);
    let flat: Vec<DocumentSymbol> = match &response {
        DocumentSymbolResponse::Nested(symbols) => symbols.clone(),
        DocumentSymbolResponse::Flat(_) => unreachable!("fixture is nested"),
    };

    /// The pre-optimisation shape: two hash maps per sibling group regardless of duplicates.
    fn always_maps(symbols: &[DocumentSymbol]) -> Vec<String> {
        let mut totals = std::collections::HashMap::<&str, usize>::new();
        for symbol in symbols {
            *totals.entry(symbol.name.as_str()).or_default() += 1;
        }
        let mut seen = std::collections::HashMap::<&str, usize>::new();
        symbols
            .iter()
            .map(|symbol| {
                if totals.get(symbol.name.as_str()).copied().unwrap_or(0) < 2 {
                    return symbol.name.clone();
                }
                let index = seen.entry(symbol.name.as_str()).or_default();
                let rendered = format!("{}[{index}]", symbol.name);
                *index += 1;
                rendered
            })
            .collect()
    }

    /// Both sides visit every sibling group in the tree, so only the naming differs.
    fn walk(symbols: &[DocumentSymbol], name: &impl Fn(&[DocumentSymbol]) -> Vec<String>) -> usize {
        let mut total = name(symbols).iter().map(String::len).sum();
        for symbol in symbols {
            total += walk(&symbol.children, name);
        }
        total
    }

    reporter.case("E2 name sibling groups [before]", 1, || {
        black_box(walk(&flat, &always_maps));
    });

    reporter.case("E2 name sibling groups [after]", 1, || {
        black_box(walk(&flat, &disambiguate));
    });

    reporter.case("E2 whole build_tree [after]", 1, || {
        black_box(build_tree(response.clone()));
    });
}

// ---------------------------------------------------------------------------------------------
// B6: URI encoding
// ---------------------------------------------------------------------------------------------

/// Built at least three times per request against a file before B5 cached it.
fn bench_uri_from_path(reporter: &mut Reporter) {
    let plain = PathBuf::from(if cfg!(windows) {
        r"C:\Users\dev\project\src\Services\PlayerService.luau"
    } else {
        "/home/dev/project/src/Services/PlayerService.luau"
    });
    let escaped = plain.with_file_name("Player Service (v2).luau");

    for (label, path) in [("unescaped", &plain), ("escaped", &escaped)] {
        reporter.case(&format!("B6 uri encode, {label} [before]"), 1, || {
            // The pre-optimisation shape: one heap allocation per escaped byte.
            let text = path.to_str().unwrap().replace('\\', "/");
            let mut encoded = String::from("file:///");
            for byte in text.bytes() {
                if byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
                {
                    encoded.push(byte as char);
                } else {
                    encoded.push_str(&format!("%{byte:02X}"));
                }
            }
            black_box(encoded);
        });

        reporter.case(&format!("B6 uri encode, {label} [after]"), 1, || {
            black_box(uri::from_path(path).unwrap());
        });
    }
}

// ---------------------------------------------------------------------------------------------
// A7: line slicing
// ---------------------------------------------------------------------------------------------

/// `include_body` renders every matching symbol out of the same file, and each render used to
/// re-split the whole file into lines.
fn bench_line_slicing(reporter: &mut Reporter) {
    let content = luau_source(SYMBOLS_PER_FILE);
    let spans: Vec<(usize, usize)> = (0..SYMBOLS_PER_FILE)
        .map(|index| (index * 4, index * 4 + 3))
        .collect();

    reporter.case(
        "A7 symbol bodies from one file [before]",
        spans.len(),
        || {
            let mut total = 0usize;
            for (start, end) in &spans {
                // The pre-optimisation shape: rebuild the whole line vector per rendered symbol.
                let lines: Vec<&str> = content.lines().collect();
                let to = (*end).min(lines.len().saturating_sub(1));
                total += lines[*start..=to].join("\n").len();
            }
            black_box(total);
        },
    );

    reporter.case(
        "A7 symbol bodies from one file [after]",
        spans.len(),
        || {
            let index = LineIndex::new(&content);
            let mut total = 0usize;
            for (start, end) in &spans {
                total += index.slice(*start, *end).len();
            }
            black_box(total);
        },
    );
}

// ---------------------------------------------------------------------------------------------
// A1: literal pre-filter
// ---------------------------------------------------------------------------------------------

/// The pre-filter's whole value is how many `documentSymbol` round trips it removes, so the case
/// reports the rejection rate next to the scan cost.
fn bench_literal_prefilter(reporter: &mut Reporter, fixture: &Fixture) {
    let files = fixture.luau_files();
    let bytes: usize = files
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len() as usize)
        .sum();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // The real thing, disk read included, which is what the scan actually pays.
    let survivors = runtime
        .block_on(prefilter_by_literal(files.clone(), Some("GetPlayerMaid")))
        .unwrap()
        .len();
    reporter.note(format!(
        "A1 prefilter keeps {survivors}/{} files ({} KiB read) for a name no file defines, so \
         that many documentSymbol round trips are never issued",
        files.len(),
        bytes / 1024
    ));

    reporter.case("A1 prefilter, name absent", files.len(), || {
        let kept = runtime
            .block_on(prefilter_by_literal(files.clone(), Some("GetPlayerMaid")))
            .unwrap();
        black_box(kept.len());
    });

    reporter.case("A1 prefilter, name in every file", files.len(), || {
        let kept = runtime
            .block_on(prefilter_by_literal(files.clone(), Some("Service0")))
            .unwrap();
        black_box(kept.len());
    });
}

// ---------------------------------------------------------------------------------------------
// D1: project walk
// ---------------------------------------------------------------------------------------------

/// The walk that every project-wide `find_symbol` with no `relative_path` starts with.
fn bench_project_walk(reporter: &mut Reporter, fixture: &Fixture) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let handle = LanguageServerHandle::new(fixture.project.clone(), Settings::default());
    let root = fixture.project.root().to_path_buf();

    reporter.case("D1 walk project for .luau [before]", FIXTURE_FILES, || {
        // The pre-optimisation shape: hidden(false) with no filter_entry, so .git is descended.
        let mut builder = ignore::WalkBuilder::new(&root);
        builder
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .require_git(false)
            .follow_links(false);

        let mut found: Vec<PathBuf> = Vec::new();
        for entry in builder.build().filter_map(Result::ok) {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.into_path();
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension == "luau" || extension == "lua")
            {
                found.push(path);
            }
        }
        found.sort();
        black_box(found.len());
    });

    reporter.case("D1 walk project for .luau [after]", FIXTURE_FILES, || {
        let found = runtime.block_on(handle.resolve_luau_files(None)).unwrap();
        black_box(found.len());
    });

    let subtree = fixture.project.root().join("src").join("Package3");
    reporter.case("A5 walk one subtree [after]", 1, || {
        let found = runtime
            .block_on(handle.resolve_luau_files(Some(&subtree)))
            .unwrap();
        black_box(found.len());
    });
}

// ---------------------------------------------------------------------------------------------
// D3 and D4: pattern search
// ---------------------------------------------------------------------------------------------

/// The common case is a pattern that matches nothing, where every line index built is a line
/// index thrown away.
fn bench_pattern_search(reporter: &mut Reporter, fixture: &Fixture) {
    let tools = FileTools::new(fixture.project.clone(), Settings::default());
    let root = fixture.project.root().to_path_buf();
    let pattern = "ThisIdentifierIsNowhereInTheFixture";

    reporter.case("D3 search, no match [before]", FIXTURE_FILES, || {
        // The pre-optimisation shape: relativize every file, then build both line structures
        // before the regex has run once.
        let regex = regex::RegexBuilder::new(pattern)
            .multi_line(true)
            .dot_matches_new_line(true)
            .build()
            .unwrap();
        let mut hits = 0usize;
        for entry in ignore::WalkBuilder::new(&root)
            .hidden(false)
            .require_git(false)
            .build()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let relative = relative.to_string_lossy().into_owned();
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| matches!(extension, "luau" | "lua" | "luaurc"))
            {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = contents.lines().collect();
            let mut offsets = vec![0usize];
            for (index, byte) in contents.bytes().enumerate() {
                if byte == b'\n' {
                    offsets.push(index + 1);
                }
            }
            black_box((&lines, &offsets, &relative));
            hits += regex.find_iter(&contents).count();
        }
        black_box(hits);
    });

    let request = |pattern: &'static str| PatternSearchRequest {
        pattern,
        relative_path: ".",
        context_lines_before: 2,
        context_lines_after: 2,
        paths_include_glob: None,
        paths_exclude_glob: None,
        restrict_to_code_files: true,
        max_matches: 200,
    };

    reporter.case("D3 search, no match [after]", FIXTURE_FILES, || {
        let result = tools
            .search_for_pattern(request("ThisIdentifierIsNowhereInTheFixture"))
            .unwrap();
        black_box(result.matches.len());
    });

    reporter.case("D3 search, many matches [after]", FIXTURE_FILES, || {
        let result = tools
            .search_for_pattern(request("function Service"))
            .unwrap();
        black_box(result.matches.len());
    });
}

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

struct Fixture {
    _dir: tempfile::TempDir,
    project: Project,
}

impl Fixture {
    fn build() -> Self {
        let dir = tempfile::tempdir().expect("fixture directory");
        let root = dir.path();
        std::fs::create_dir_all(root.join(".biskit")).unwrap();

        // A repository-shaped .git, which the walk has no reason to descend into.
        let objects = root.join(".git").join("objects");
        for bucket in 0..16 {
            let bucket_dir = objects.join(format!("{bucket:02x}"));
            std::fs::create_dir_all(&bucket_dir).unwrap();
            for object in 0..(FIXTURE_GIT_OBJECTS / 16) {
                std::fs::write(bucket_dir.join(format!("{object:038x}")), b"blob").unwrap();
            }
        }
        std::fs::write(root.join(".git").join("HEAD"), b"ref: refs/heads/main\n").unwrap();

        let source = luau_source(SYMBOLS_PER_FILE);
        for index in 0..FIXTURE_FILES {
            let module = root
                .join("src")
                .join(format!("Package{}", index % 24))
                .join("Services");
            std::fs::create_dir_all(&module).unwrap();
            std::fs::write(module.join(format!("Service{index}.luau")), &source).unwrap();
        }

        let project = Project::open(root).expect("fixture project");
        Self { _dir: dir, project }
    }

    fn luau_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        collect_luau(self.project.root(), &mut found);
        found
    }
}

fn collect_luau(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_luau(&path, out);
        } else if path.extension().is_some_and(|value| value == "luau") {
            out.push(path);
        }
    }
}

fn luau_source(symbols: usize) -> String {
    let mut text = String::new();
    for index in 0..symbols {
        text.push_str(&format!(
            "local Service{index} = {{}}\n\
             function Service{index}:method{}(player: Player): number\n    \
             return player.UserId + {index}\nend\n",
            index % 8
        ));
    }
    text
}

fn document_symbols(count: usize) -> DocumentSymbolResponse {
    let span = |line: u32| Range {
        start: Position { line, character: 0 },
        end: Position {
            line: line + 3,
            character: 3,
        },
    };
    let symbols = (0..count)
        .map(|index| {
            let line = (index * 4) as u32;
            DocumentSymbol {
                name: format!("Service{index}"),
                detail: Some(format!("(self: Service{index}, player: Player) -> number")),
                kind: 12,
                range: span(line),
                selection_range: span(line),
                children: (0..4)
                    .map(|child| DocumentSymbol {
                        name: format!("Service{index}:method{child}"),
                        detail: Some("(player: Player) -> number".to_string()),
                        kind: 6,
                        range: span(line),
                        selection_range: span(line),
                        children: Vec::new(),
                    })
                    .collect(),
            }
        })
        .collect();
    DocumentSymbolResponse::Nested(symbols)
}

fn flat_symbols(count: usize) -> Vec<SymbolNode> {
    let tree = build_tree(document_symbols(count));
    let mut nodes = Vec::new();
    for root in &tree {
        root.walk(&mut |node| nodes.push(node.clone()));
    }
    nodes
}

// ---------------------------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------------------------

/// Each case runs until it has spent this long, so short cases still gather enough samples to be
/// stable and long cases still finish.
const SAMPLE_BUDGET: Duration = Duration::from_millis(350);
const WARMUP_ROUNDS: usize = 3;
const MAX_SAMPLES: usize = 2_000;

struct Reporter {
    filter: String,
    cases: usize,
    notes: Vec<String>,
}

impl Reporter {
    fn new(filter: String) -> Self {
        Self {
            filter,
            cases: 0,
            notes: Vec::new(),
        }
    }

    fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    /// `units` is how many logical items one round covers, so a case that batches work still
    /// reports a per-item figure next to the per-round one.
    fn case(&mut self, name: &str, units: usize, mut body: impl FnMut()) {
        if !self.filter.is_empty() && !name.contains(&self.filter) {
            return;
        }
        if self.cases == 0 {
            println!("{:<44} {:>12}  {:>12}", "case", "median", "per unit");
        }

        for _ in 0..WARMUP_ROUNDS {
            body();
        }

        let mut samples: Vec<Duration> = Vec::new();
        let deadline = Instant::now() + SAMPLE_BUDGET;
        while (Instant::now() < deadline || samples.len() < 5) && samples.len() < MAX_SAMPLES {
            let started = Instant::now();
            body();
            samples.push(started.elapsed());
        }

        samples.sort();
        let median = samples[samples.len() / 2].as_secs_f64();
        println!(
            "{name:<44} {:>12}  {:>12}  ({} samples)",
            format_time(median),
            format_time(median / units.max(1) as f64),
            samples.len()
        );
        self.cases += 1;
    }

    fn finish(self) {
        for note in &self.notes {
            println!("\nnote: {note}");
        }
        println!("\n{} cases", self.cases);
    }
}

fn format_time(seconds: f64) -> String {
    let nanos = seconds * 1e9;
    if nanos < 1_000.0 {
        format!("{nanos:.1}ns")
    } else if nanos < 1_000_000.0 {
        format!("{:.2}us", nanos / 1e3)
    } else {
        format!("{:.2}ms", nanos / 1e6)
    }
}
