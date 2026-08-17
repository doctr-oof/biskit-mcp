//! End-to-end timing against a real project and a real `luau-lsp`.
//!
//! Opt-in, because it starts a language server and can take a minute:
//!
//! ```text
//! BISKIT_BENCH_PROJECT=/path/to/checkout cargo bench --bench end_to_end
//! ```
//!
//! What it is here to answer is the one claim the micro-benchmarks cannot: what a
//! `documentSymbol` round trip actually costs, and therefore what skipping one is worth. Every
//! other measurement in `hot_paths.rs` is denominated in that number.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use biskit_mcp::config::Settings;
use biskit_mcp::lsp::queries::{FindSymbolRequest, SymbolQuery, prefilter_by_literal};
use biskit_mcp::lsp::session::LanguageServerHandle;
use biskit_mcp::project::Project;

/// Files sampled for the per-round-trip measurement. The whole project would be accurate and
/// would also take as long as the regression this change exists to remove.
const ROUND_TRIP_SAMPLE: usize = 40;
/// A name chosen so that no file in any project defines it, which is the exploratory case.
const ABSENT_NAME: &str = "NoSuchSymbolExistsAnywhereHere";

fn main() {
    let Ok(root) = std::env::var("BISKIT_BENCH_PROJECT") else {
        println!("skipped: set BISKIT_BENCH_PROJECT to a checkout to run the end-to-end bench");
        return;
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if let Err(error) = runtime.block_on(run(&root)) {
        println!("end-to-end bench failed: {error:#}");
    }
}

async fn run(root: &str) -> anyhow::Result<()> {
    let project = Project::open(root)?;
    let settings = Settings::load(&project.settings_path(), &project.local_settings_path())?;
    let handle = LanguageServerHandle::new(project.clone(), settings);

    println!("project           {root}");

    // F1: what the first LSP-backed tool call used to pay for before the warm-up existed.
    let started = Instant::now();
    let session = handle.session().await?;
    println!("startup           {:?}", started.elapsed());

    let files = handle.resolve_luau_files(None).await?;
    println!("luau files        {}", files.len());

    // A1: how much of the candidate set a literal pre-filter removes, and what it cost to do it.
    let started = Instant::now();
    let survivors = prefilter_by_literal(files.clone(), Some(ABSENT_NAME)).await?;
    let prefilter = started.elapsed();
    println!(
        "prefilter         {:?} for {} files, {} survive",
        prefilter,
        files.len(),
        survivors.len()
    );

    // The number everything else is denominated in.
    let sample: Vec<PathBuf> = files.iter().take(ROUND_TRIP_SAMPLE).cloned().collect();
    let started = Instant::now();
    let mut symbols_seen = 0usize;
    for path in &sample {
        if let Ok((symbols, _)) = session.document_symbols(path).await {
            symbols_seen += symbols.len();
        }
    }
    let sampled = started.elapsed();
    let per_file = sampled / sample.len().max(1) as u32;
    println!(
        "documentSymbol    {per_file:?} per file over {} files ({symbols_seen} top-level symbols)",
        sample.len()
    );

    let projected = per_file * files.len() as u32;
    println!(
        "\nprojected scan of every file   {projected:?}\nprefilter over every file      \
         {prefilter:?}"
    );

    // The tool call itself, with the pre-filter in place. The sample loop above left its files
    // open, and luau-lsp answers faster for a document it has already analysed, so treat these as
    // warm-session figures rather than as the cost of the very first query.
    for (label, name, substring) in [
        ("absent name", ABSENT_NAME.to_string(), false),
        ("substring query", "Service".to_string(), true),
    ] {
        let kept = prefilter_by_literal(files.clone(), Some(name.as_str())).await?;
        println!(
            "\n{label:<16}  pre-filter keeps {}/{} files, so at most that many round trips",
            kept.len(),
            files.len()
        );

        let started = Instant::now();
        let result = SymbolQuery::new(&handle)
            .find_symbol(FindSymbolRequest {
                name_path: name,
                relative_path: None,
                depth: 0,
                include_body: false,
                include_detail: false,
                include_kinds: Vec::new(),
                exclude_kinds: Vec::new(),
                substring_matching: substring,
                max_matches: 50,
            })
            .await?;
        println!(
            "find_symbol {label:<16} {:?}, {} files matched",
            started.elapsed(),
            result.symbols.len()
        );
        black_box(result);
    }

    handle.stop().await;
    Ok(())
}
