//! Checks that the literal pre-filter changes only the cost of `find_symbol`, never its answer.
//!
//! ```text
//! BISKIT_TEST_PROJECT=/path/to/checkout cargo test --test prefilter_equivalence -- --ignored
//! ```

use std::collections::BTreeSet;

use biskit_mcp::config::Settings;
use biskit_mcp::lsp::queries::{FindSymbolRequest, SymbolQuery};
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

fn flatten(result: &biskit_mcp::lsp::queries::SymbolSearchResult) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (file, symbols) in &result.symbols {
        for symbol in symbols {
            found.insert(format!(
                "{file}::{}",
                symbol.name_path.as_deref().unwrap_or("<unnamed>")
            ));
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs BISKIT_TEST_PROJECT and a real luau-lsp"]
async fn the_prefilter_never_changes_which_symbols_are_found() {
    let Ok(root) = std::env::var("BISKIT_TEST_PROJECT") else {
        panic!("set BISKIT_TEST_PROJECT to a checkout containing Luau source");
    };

    let project = Project::open(&root).expect("project root");
    let settings = Settings::load(&project.settings_path(), &project.local_settings_path())
        .expect("project settings");
    let handle = LanguageServerHandle::new(project.clone(), settings);
    let files = handle
        .resolve_luau_files(None)
        .await
        .expect("project scan")
        .iter()
        .map(|path| project.relativize(path).expect("relative path"))
        .collect::<Vec<_>>();
    assert!(!files.is_empty(), "{root} contains no Luau source");

    let existing = first_defined_symbol(&handle, &files)
        .await
        .expect("no symbol found anywhere in the project to compare against");
    println!("comparing against existing symbol {existing}");

    let cases = [
        ("NoSuchSymbolExistsAnywhereHere", false),
        (existing.as_str(), false),
        ("Service", true),
        ("get", true),
    ];

    for (name, substring) in cases {
        let filtered = SymbolQuery::new(&handle)
            .find_symbol(request(name, substring))
            .await
            .unwrap_or_else(|error| panic!("project-wide {name:?} failed: {error:#}"));
        assert!(
            !filtered.truncated,
            "{name:?} was truncated; raise max_matches for a meaningful comparison"
        );

        let mut unfiltered = BTreeSet::new();
        for file in &files {
            let mut single = request(name, substring);
            single.relative_path = Some(file.clone());
            let Ok(result) = SymbolQuery::new(&handle).find_symbol(single).await else {
                continue;
            };
            unfiltered.extend(flatten(&result));
        }

        let filtered = flatten(&filtered);
        let missing: Vec<&String> = unfiltered.difference(&filtered).collect();
        let extra: Vec<&String> = filtered.difference(&unfiltered).collect();

        assert!(
            missing.is_empty(),
            "{name:?}: the pre-filter dropped {} result(s) the unfiltered scan found: {missing:?}",
            missing.len()
        );
        assert!(
            extra.is_empty(),
            "{name:?}: the pre-filtered scan reported {} result(s) the unfiltered scan did not: \
             {extra:?}",
            extra.len()
        );
        println!("{name:?}: {} results, identical both ways", filtered.len());
    }

    handle.stop().await;
}

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
