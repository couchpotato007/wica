//! wica - "what is it called?" Find CLI tools by describing what they do.

mod embed;
mod fuzzy;
mod index;
mod packages;
mod render;
mod search;
mod sources;

use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Find the CLI tool you forgot the name of.
///
/// Example: wica render markdown
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// What you want to do, e.g. "search in files"
    query: Vec<String>,

    /// Also show tools that are not installed
    #[arg(short, long)]
    all: bool,

    /// Number of results
    #[arg(short = 'n', long, default_value_t = 10)]
    limit: usize,

    /// Hide the usage example under each result
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Show how each result was scored
    #[arg(long, hide = true)]
    debug: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download the latest tldr pages and rebuild the index
    Update,
    /// Rebuild the index from local sources (PATH, man pages, tldr cache)
    Index,
    /// Show which package managers wica detected and where to configure them
    Managers,
    /// Find a tool by a name you half remember: wica name deer
    #[command(visible_alias = "n")]
    Name {
        #[arg(required = true)]
        name: Vec<String>,
        /// Number of results
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Don't search the package managers (faster)
        #[arg(short, long)]
        local: bool,
    },
    /// Show all tldr examples for a tool: wica tldr tar
    Tldr {
        #[arg(required = true)]
        tool: Vec<String>,
    },
    /// Add your own keywords to a tool: wica tag rg grep search text
    Tag {
        tool: String,
        #[arg(required = true)]
        words: Vec<String>,
    },
}

fn main() -> Result<()> {
    // Exit quietly when output is piped into e.g. `head`, like other CLI tools,
    // instead of panicking on a closed pipe.
    // SAFETY: called before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Update) => {
            let zip = index::tldr_zip()?;
            eprintln!("Downloading tldr pages...");
            sources::tldr::download(&zip)?;
            let model_dir = embed::model_dir(&index::cache_dir()?);
            if embed::Embedder::load(&model_dir)?.is_none() {
                eprintln!("Downloading embedding model {} ...", embed::model_name());
                embed::download(&model_dir)?;
            }
            rebuild()
        }
        Some(Cmd::Index) => rebuild(),
        Some(Cmd::Managers) => list_managers(),
        Some(Cmd::Name { name, limit, local }) => find_name(&name.join(" "), limit, local),
        Some(Cmd::Tldr { tool }) => show_page(&tool),
        Some(Cmd::Tag { tool, words }) => {
            let file = sources::tags::add(&tool, &words)?;
            eprintln!("Tagged {tool} with: {} ({})", words.join(" "), file.display());
            Ok(())
        }
        None if cli.query.is_empty() => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            Ok(())
        }
        None => run_search(&cli),
    }
}

fn list_managers() -> Result<()> {
    for m in packages::configured()? {
        let status = if packages::is_available(&m) { "found" } else { "-" };
        let lookup = if m.lookup.is_empty() { "no package lookup" } else { "package lookup" };
        println!(" {:<8} {:<6} {}   ({lookup})", m.name, status, m.install);
    }
    if let Some(f) = packages::user_config() {
        println!("\nAdd or override managers in {}", f.display());
    }
    Ok(())
}

fn load_embedder() -> Result<Option<embed::Embedder>> {
    embed::Embedder::load(&embed::model_dir(&index::cache_dir()?))
}

fn rebuild() -> Result<()> {
    let start = Instant::now();
    let embedder = load_embedder()?;
    let index = index::build(embedder.as_ref())?;
    index::save(&index)?;
    let installed = index.tools.iter().filter(|t| t.installed).count();
    eprintln!(
        "Indexed {} tools ({installed} installed) in {:.1?}",
        index.tools.len(),
        start.elapsed()
    );
    Ok(())
}

/// Rebuild daily so newly installed tools show up.
const INDEX_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

fn load_index() -> Result<index::Index> {
    index::load_or_build(INDEX_MAX_AGE, load_embedder()?.as_ref())
}

/// Tools whose name (or package, or project name) is close to `query`, best first.
fn name_matches<'a>(index: &'a index::Index, query: &str) -> Vec<(&'a index::Tool, Option<String>, f32)> {
    let mut matches: Vec<_> = index
        .tools
        .iter()
        .filter_map(|t| {
            let own = fuzzy::score(query, &t.name);
            let (other, other_score) = t
                .other_names()
                .into_iter()
                .map(|n| {
                    let s = fuzzy::score(query, &n);
                    (n, s)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or_default();
            // Matching the real name beats matching a package or project name.
            let (via, score) = if other_score - 0.05 > own { (Some(other), other_score - 0.05) } else { (None, own) };
            // Prefer what's installed and what has docs when names tie.
            let bonus = if t.installed { 0.03 } else { 0.0 } + if t.summary().is_empty() { 0.0 } else { 0.01 };
            (score >= fuzzy::MIN_SCORE).then_some((t, via, score + bonus))
        })
        .collect();
    matches.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.name.len().cmp(&b.0.name.len())));
    // A close match makes the far-fetched ones noise.
    let best = matches.first().map_or(0.0, |m| m.2);
    matches.retain(|m| m.2 >= best - 0.25);
    matches
}

fn find_name(query: &str, limit: usize, local: bool) -> Result<()> {
    let index = load_index()?;
    let style = render::Style::detect();
    let mut packages = packages::Packages::detect()?;
    let mut matches = name_matches(&index, query);
    matches.truncate(limit);

    let shown: Vec<(&index::Tool, Option<&str>)> = matches.iter().map(|(t, via, _)| (*t, via.as_deref())).collect();
    let missing: Vec<&str> = shown.iter().filter(|(t, _)| !t.installed).map(|(t, _)| t.name.as_str()).collect();
    let install = packages.hints(&missing);
    render::names(&shown, &style, &install);

    // Packages wica has no docs for (e.g. `tealdeer` when it isn't installed).
    let mut extra = Vec::new();
    if !local && query.chars().count() >= 3 {
        let known = |name: &str| {
            index.tools.iter().any(|t| t.name == name || t.other_names().iter().any(|n| n == name))
                || matches.iter().any(|(t, _, _)| t.name == name)
        };
        let mut found: Vec<(f32, String, packages::Found)> = packages
            .search(query)
            .into_iter()
            .map(|(m, f)| (fuzzy::score(query, &f.name), m, f))
            .filter(|(s, _, f)| *s >= fuzzy::MIN_SCORE && !known(&f.name))
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.name.len().cmp(&b.2.name.len())));
        found.dedup_by(|a, b| a.2.name == b.2.name);
        found.truncate(limit);
        extra = found.into_iter().map(|(_, m, f)| (m, f)).collect();
    }
    if !extra.is_empty() {
        let install = packages.install_hints(&extra);
        if !shown.is_empty() {
            println!("\nPackages:\n");
        }
        render::packages(&extra, &style, &install);
    }
    if shown.is_empty() && extra.is_empty() {
        eprintln!("No tool or package named like \"{query}\"");
        std::process::exit(1);
    }
    Ok(())
}

fn show_page(words: &[String]) -> Result<()> {
    let key = words.join("-").to_lowercase();
    let index = load_index()?;
    let style = render::Style::detect();
    let zip = index::tldr_zip()?;
    let tool = index.tools.iter().find(|t| t.name.replace(' ', "-").eq_ignore_ascii_case(&key));

    let page = if zip.exists() { sources::tldr::read_page(&zip, &key)? } else { None };
    match (&page, tool) {
        (Some((platform, text)), _) => {
            // `vi` is an alias page that only says "see vim": show vim's page.
            let alias = sources::tldr::parse(&key, text).and_then(|p| p.alias_of);
            match alias.as_deref().and_then(|a| Some((a, sources::tldr::read_page(&zip, a).ok()??))) {
                Some((target, (platform, text))) => {
                    println!("\n  {}", style.paint_dim(&format!("{} is an alias of {target}", words.join(" "))));
                    render::page(&text, &platform, &style);
                }
                None => render::page(text, platform, &style),
            }
        }
        (None, Some(t)) if !t.summary().is_empty() => {
            println!("\n  {}\n\n  {}\n\n  {}\n", t.name, t.summary(), style.paint_dim("No tldr page; see the man page."));
        }
        _ => {
            if !zip.exists() {
                eprintln!("hint: run `wica update` once to download tldr pages\n");
            }
            let query = words.join(" ");
            let close: Vec<String> = name_matches(&index, &query).into_iter().take(5).map(|(t, _, _)| t.name.clone()).collect();
            if close.is_empty() {
                eprintln!("No page for \"{query}\".");
            } else {
                eprintln!("No page for \"{query}\". Did you mean: {}?", close.join(", "));
            }
            std::process::exit(1);
        }
    }
    if let Some(t) = tool.filter(|t| !t.installed) {
        let install = packages::Packages::detect()?.hints(&[t.name.as_str()]);
        render::names(&[(t, t.package.as_deref())], &style, &install);
        println!();
    }
    Ok(())
}

fn run_search(cli: &Cli) -> Result<()> {
    let t0 = Instant::now();
    let embedder = load_embedder()?;
    let t_model = t0.elapsed();
    let index = index::load_or_build(INDEX_MAX_AGE, embedder.as_ref())?;
    let t_index = t0.elapsed() - t_model;
    if !index::tldr_zip()?.exists() || embedder.is_none() {
        eprintln!("hint: run `wica update` once to download tldr pages and the search model\n");
    }
    let tags = sources::tags::load();
    let query = cli.query.join(" ");
    let searcher = search::Searcher::new();
    let results = searcher.search(&index, &tags, &query, embedder.as_ref(), cli.all, cli.limit);
    if cli.debug {
        eprintln!("timing: model {t_model:.1?}, index {t_index:.1?}, search {:.1?}", t0.elapsed() - t_model - t_index);
    }

    if cli.debug {
        for h in results.shown.iter().chain(&results.suggestions) {
            eprintln!("{:>6.3}  rel {:.2}  sem {:.3}  kw {:.3}  {}", h.score, h.relevance, h.parts.0, h.parts.1, h.tool.name);
        }
        eprintln!();
    }
    if results.shown.is_empty() && results.suggestions.is_empty() {
        eprintln!("No tools found for \"{query}\"");
        std::process::exit(1);
    }
    let style = render::Style::detect();
    let missing: Vec<&str> = results
        .shown
        .iter()
        .chain(&results.suggestions)
        .filter(|h| !h.tool.installed)
        .map(|h| h.tool.name.as_str())
        .collect();
    let install = match packages::Packages::detect() {
        Ok(mut p) if !missing.is_empty() => p.hints(&missing),
        Ok(_) => Default::default(),
        Err(e) => {
            eprintln!("warning: {e:#}");
            Default::default()
        }
    };
    // A single word may be a half-remembered name rather than a task.
    if !query.contains(' ') {
        let shown: Vec<&str> = results.shown.iter().chain(&results.suggestions).map(|h| h.tool.name.as_str()).collect();
        let names: Vec<String> = name_matches(&index, &query)
            .into_iter()
            .filter(|(t, _, s)| *s >= 0.7 && !shown.contains(&t.name.as_str()))
            .take(5)
            .map(|(t, via, _)| via.map_or(t.name.clone(), |v| format!("{} ({v})", t.name)))
            .collect();
        if !names.is_empty() {
            println!("{} {}\n", style.paint_dim("Names like this:"), names.join(", "));
        }
    }
    if results.shown.is_empty() {
        println!("No installed tool fits. You could install:\n");
    } else {
        render::hits(&results.shown, &style, !cli.quiet, &install);
        if !results.suggestions.is_empty() {
            println!("\nNot installed, but a better fit:\n");
        }
    }
    render::hits(&results.suggestions, &style, !cli.quiet, &install);

    if !cli.quiet && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        if let Some(first) = results.shown.first().or(results.suggestions.first()) {
            println!("\n{}", style.paint_dim(&format!("All examples: wica tldr {}", first.tool.name)));
        }
    }
    Ok(())
}
