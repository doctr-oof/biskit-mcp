//! Checks that the persistent symbol index changes only the cost of a symbol query, never its
//! answer.
//!
//! The cache rests on one assumption: a file whose size and modification time have not moved still
//! has the symbol tree that was stored for it. This test is what stops that from being taken on
//! faith. It runs a project-wide `find_symbol` against a cold cache, writes the index, opens a
//! second handle that can only have loaded the index from disk, and compares the two answers.
//!
//! It also checks the other half: a file edited after it was indexed must not be answered from the
//! stored tree.
//!
//! Ignored by default because it needs a real checkout and a real `luau-lsp`:
//!
//! ```text
//! BISKIT_TEST_PROJECT=/path/to/checkout cargo test --test symbol_cache_equivalence -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
use std::time::Instant;

use biskit_mcp::config::Settings;
use biskit_mcp::lsp::cache::SymbolCache;
use biskit_mcp::lsp::queries::{FindSymbolRequest, SymbolQuery, SymbolSearchResult};
use biskit_mcp::lsp::session::LanguageServerHandle;
use biskit_mcp::project::Project;

fn request(name: &str, substring: bool) -> FindSymbolRequest {
    FindSymbolRequest {
        name_path: name.to_string(),
        relative_path: None,
        depth: 0,
        include_body: false,
        include_detail: false,
        include_locals: false,
        include_kinds: Vec::new(),
        exclude_kinds: Vec::new(),
        substring_matching: substring,
        max_matches: 5_000,
    }
}

/// Every `file::name_path` pair a result reports, flattened so two passes compare directly.
fn flatten(result: &SymbolSearchResult) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (file, symbols) in &result.symbols {
        for symbol in symbols {
            found.insert(format!(
                "{file}::{} @{}-{}",
                symbol.name_path.as_deref().unwrap_or("<unnamed>"),
                symbol.start_line,
                symbol.end_line
            ));
        }
    }
    found
}

fn open() -> (Project, Settings) {
    let Ok(root) = std::env::var("BISKIT_TEST_PROJECT") else {
        panic!("set BISKIT_TEST_PROJECT to a checkout containing Luau source");
    };
    let project = Project::open(&root).expect("project root");
    let settings = Settings::load(&project.settings_path(), &project.local_settings_path())
        .expect("project settings");
    assert!(
        settings.tools.symbol_cache,
        "this project has tools.symbol_cache turned off"
    );
    (project, settings)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs BISKIT_TEST_PROJECT and a real luau-lsp"]
async fn a_warm_index_answers_exactly_what_the_language_server_did() {
    let (project, settings) = open();
    SymbolCache::clear(&project).expect("clear the cache");

    // The name has to exist, because an answer of nothing is the one answer a broken cache would
    // also give.
    let cold_handle = LanguageServerHandle::new(project.clone(), settings.clone());
    let files = cold_handle
        .resolve_luau_files(None)
        .await
        .expect("project scan")
        .iter()
        .map(|path| project.relativize(path).expect("relative path"))
        .collect::<Vec<_>>();
    assert!(!files.is_empty(), "the project contains no Luau source");

    let existing = first_defined_symbol(&cold_handle, &files)
        .await
        .expect("no symbol found anywhere in the project to compare against");
    println!("comparing against existing symbol {existing}");

    let cases = [(existing.as_str(), false), ("Service", true), ("get", true)];

    let mut cold_answers = Vec::new();
    let started = Instant::now();
    for (name, substring) in cases {
        let result = SymbolQuery::new(&cold_handle)
            .find_symbol(request(name, substring))
            .await
            .unwrap_or_else(|error| panic!("cold {name:?} failed: {error:#}"));
        assert!(
            !result.truncated,
            "{name:?} was truncated; raise max_matches for a meaningful comparison"
        );
        cold_answers.push(flatten(&result));
    }
    let cold_elapsed = started.elapsed();

    let indexed = cold_handle.symbol_cache().entry_count().await;
    assert!(indexed > 0, "the cold pass indexed nothing");
    cold_handle.stop().await;

    // A second handle shares nothing with the first but the files on disk, so anything it answers
    // without asking the language server came out of the index that was just written.
    let warm_handle = LanguageServerHandle::new(project.clone(), settings);
    assert_eq!(
        warm_handle.symbol_cache().entry_count().await,
        indexed,
        "the written index did not come back"
    );

    let started = Instant::now();
    for ((name, substring), cold) in cases.into_iter().zip(cold_answers) {
        let warm = SymbolQuery::new(&warm_handle)
            .find_symbol(request(name, substring))
            .await
            .unwrap_or_else(|error| panic!("warm {name:?} failed: {error:#}"));
        let warm = flatten(&warm);

        let missing: Vec<&String> = cold.difference(&warm).collect();
        let extra: Vec<&String> = warm.difference(&cold).collect();
        assert!(
            missing.is_empty(),
            "{name:?}: the warm pass lost {} result(s): {missing:?}",
            missing.len()
        );
        assert!(
            extra.is_empty(),
            "{name:?}: the warm pass invented {} result(s): {extra:?}",
            extra.len()
        );
        println!("{name:?}: {} results, identical both ways", warm.len());
    }
    let warm_elapsed = started.elapsed();

    println!(
        "{indexed} files indexed; cold {}ms, warm {}ms",
        cold_elapsed.as_millis(),
        warm_elapsed.as_millis()
    );

    warm_handle.stop().await;
    SymbolCache::clear(&project).expect("clear the cache");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs BISKIT_TEST_PROJECT and a real luau-lsp"]
async fn an_edited_file_is_not_answered_from_the_tree_stored_for_it() {
    let (project, settings) = open();
    SymbolCache::clear(&project).expect("clear the cache");

    // Written into the project because the cache keys on a project-relative path; removed at the
    // end whichever way the assertions go.
    let relative = "biskit_symbol_cache_probe.luau";
    let path = project.root().join(relative);
    std::fs::write(&path, "local ProbeBefore = 1\nreturn ProbeBefore\n").expect("write the probe");

    let handle = LanguageServerHandle::new(project.clone(), settings);
    let names = |result: &SymbolSearchResult| -> BTreeSet<String> {
        result
            .symbols
            .values()
            .flatten()
            .filter_map(|symbol| symbol.name_path.clone())
            .collect()
    };

    let outcome = async {
        let before = SymbolQuery::new(&handle)
            .find_symbol(request("ProbeBefore", false))
            .await?;
        anyhow::ensure!(
            names(&before).contains("ProbeBefore"),
            "the probe file was never indexed: {before:?}"
        );

        std::fs::write(
            &path,
            "local ProbeAfter = 1\nlocal Padding = 2\nreturn ProbeAfter\n",
        )?;

        let stale = SymbolQuery::new(&handle)
            .find_symbol(request("ProbeBefore", false))
            .await?;
        anyhow::ensure!(
            !names(&stale).contains("ProbeBefore"),
            "the edited file was answered from its stored tree: {stale:?}"
        );

        let fresh = SymbolQuery::new(&handle)
            .find_symbol(request("ProbeAfter", false))
            .await?;
        anyhow::ensure!(
            names(&fresh).contains("ProbeAfter"),
            "the edited file was not re-indexed: {fresh:?}"
        );
        anyhow::Ok(())
    }
    .await;

    let _ = std::fs::remove_file(&path);
    handle.stop().await;
    let _ = SymbolCache::clear(&project);
    outcome.expect("the cache served a stale answer");
}

/// A symbol the project actually defines, so the "name that exists" case has something to look for.
async fn first_defined_symbol(handle: &LanguageServerHandle, files: &[String]) -> Option<String> {
    for file in files.iter().take(25) {
        let Ok(overview) = SymbolQuery::new(handle)
            .symbols_overview(file, 0, false, false)
            .await
        else {
            continue;
        };
        if let Some(name) = overview
            .symbols
            .iter()
            .filter_map(|symbol| symbol.name_path.as_deref())
            .filter_map(|path| path.rsplit('/').next())
            .find(|leaf| leaf.len() > 3)
        {
            return Some(name.to_string());
        }
    }
    None
}
