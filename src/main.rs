mod embed;
mod error;
mod format;
mod index;
mod store;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use clap::{Parser, Subcommand, ValueEnum};

use embed::Embedder;
use error::Error;
use format::{Formatter, parse_slug};
use store::Store;

/// L2 distance threshold — results beyond this are too dissimilar.
/// On normalized vectors, L2 ranges from 0 (identical) to 2 (opposite).
const MAX_DISTANCE: f64 = 1.15;

#[derive(Parser)]
#[command(
    name = "stacks",
    about = "Semantic search for project documentation",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search documentation by meaning
    Search {
        /// Search query
        #[arg(num_args = 1.., required = true)]
        query: Vec<String>,

        /// Maximum number of results
        #[arg(short, default_value = "10")]
        n: usize,

        /// Show complete chunk content
        #[arg(short, long)]
        detail: bool,

        /// Skip relevance threshold, return up to -n results
        #[arg(short, long)]
        all: bool,

        /// Scope results to a file or directory
        #[arg(short, long)]
        path: Option<String>,
    },

    /// Show a specific section by slug or source path
    Show {
        /// Chunk slug or source file path
        target: String,

        /// Show all descendant content
        #[arg(short, long)]
        detail: bool,
    },

    /// Emit context primer for agent onboarding
    Prime {
        /// Install SessionStart hook for an agent platform
        #[arg(long)]
        install: Option<Platform>,
    },

    /// Browse the heading index
    Index {
        /// Scope to a file or directory
        #[arg(short, long)]
        path: Option<String>,

        /// Filter headings by keyword
        #[arg(short, long)]
        grep: Option<String>,
    },
}

#[derive(Clone, ValueEnum)]
enum Platform {
    Claude,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), Error> {
    let cwd = env::current_dir()?;
    let store = Store::find_or_create(&cwd)?;

    // prime --install doesn't need the index or embedder.
    if let Command::Prime { install } = &cli.command {
        return cmd_prime(&store, install.as_ref());
    }

    let fmt = Formatter::new();
    let on_disk = index::scan_sources(&store)?;
    let needs_search = matches!(&cli.command, Command::Search { .. });
    let needs_embed = needs_search || index::has_stale_files(&store, &on_disk)?;

    let embedder = if needs_embed {
        Some(Embedder::new()?)
    } else {
        None
    };

    if let Some(ref embedder) = embedder {
        index::ensure_fresh(&store, embedder, &on_disk)?;
    }

    match &cli.command {
        Command::Search {
            query,
            n,
            detail,
            all,
            path,
        } => {
            let query = query.join(" ");
            let opts = SearchOpts {
                query: &query,
                n: *n,
                detail: *detail,
                all: *all,
                path: path.as_deref(),
            };
            cmd_search(&store, embedder.as_ref().unwrap(), &opts, &fmt)
        }
        Command::Show { target, detail } => cmd_show(&store, target, *detail, &fmt),
        Command::Prime { .. } => unreachable!(),
        Command::Index { path, grep } => cmd_index(&store, path.as_deref(), grep.as_deref(), &fmt),
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

struct SearchOpts<'a> {
    query: &'a str,
    n: usize,
    detail: bool,
    all: bool,
    path: Option<&'a str>,
}

fn cmd_search(
    store: &Store,
    embedder: &Embedder,
    opts: &SearchOpts,
    fmt: &Formatter,
) -> Result<(), Error> {
    let query_vec = embedder.embed_one(opts.query);
    let results = store.search_similar(&query_vec, opts.n, opts.path)?;

    if results.is_empty() {
        println!("No results.");
        return Ok(());
    }

    if opts.detail {
        return cmd_search_detail(store, &results, opts.all, fmt);
    }

    // Pattern A: header listing with tree chars, grouped by adjacent file.
    // Collect filtered results first so we can compute is_last per group.
    let filtered: Vec<&(store::Chunk, f64)> = results
        .iter()
        .filter(|(_, d)| opts.all || *d <= MAX_DISTANCE)
        .collect();

    let mut i = 0;
    while i < filtered.len() {
        // Collect a group of adjacent results from the same file.
        let source = &filtered[i].0.source_path;
        let group_start = i;
        while i < filtered.len() && filtered[i].0.source_path == *source {
            i += 1;
        }
        let group = &filtered[group_start..i];

        if group_start > 0 {
            println!();
        }
        println!("{}", fmt.bold(source));
        for (j, (chunk, _)) in group.iter().enumerate() {
            let is_last = j == group.len() - 1;
            let prefix = fmt.tree_connector(is_last);
            let ancestors = store.ancestors_by_query(chunk)?;
            let crumb = fmt.breadcrumb(&ancestors, &chunk.heading);
            println!("{prefix}{crumb} {}", fmt.slug(&chunk.slug));
        }
    }

    Ok(())
}

fn cmd_show(store: &Store, target: &str, detail: bool, fmt: &Formatter) -> Result<(), Error> {
    let slug = parse_slug(target);

    if let Some(chunk) = store.chunk_by_slug(slug)? {
        if detail {
            return cmd_show_detail(store, &chunk, fmt);
        }

        let ancestors = store.ancestors_by_query(&chunk)?;
        println!("{}", fmt.box_top(&chunk.source_path));
        println!("{}", fmt.breadcrumb_nav(&ancestors, &chunk.heading));
        println!();

        if !chunk.content.is_empty() {
            println!("{}", chunk.content);
            println!();
        }

        let children = store.children_of(chunk.id)?;
        for child in &children {
            println!(
                "{} {} {}",
                fmt.dim("·"),
                fmt.bold(&child.heading),
                fmt.slug(&child.slug)
            );
        }
        return Ok(());
    }

    let roots = store.root_chunks_for_source(target)?;
    if roots.is_empty() {
        return Err(Error::ChunkNotFound(target.to_string()));
    }

    println!("{}", fmt.bold(target));
    for chunk in &roots {
        println!(
            "{} {} {}",
            fmt.dim("·"),
            fmt.bold(&chunk.heading),
            fmt.slug(&chunk.slug)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// search --detail, show --detail
// ---------------------------------------------------------------------------

fn cmd_search_detail(
    store: &Store,
    results: &[(store::Chunk, f64)],
    all: bool,
    fmt: &Formatter,
) -> Result<(), Error> {
    let mut last_source = "";
    for (chunk, distance) in results {
        if !all && *distance > MAX_DISTANCE {
            break;
        }

        let ancestors = store.ancestors_by_query(chunk)?;

        if chunk.source_path != last_source {
            println!("{}", fmt.box_top(&chunk.source_path));
            last_source = &chunk.source_path;
        }

        let crumb = fmt.breadcrumb(&ancestors, &chunk.heading);
        println!("{crumb} {}", fmt.slug(&chunk.slug));

        if !chunk.content.is_empty() {
            println!();
            println!("{}", chunk.content);
        }
        println!();
    }

    Ok(())
}

fn cmd_show_detail(store: &Store, chunk: &store::Chunk, fmt: &Formatter) -> Result<(), Error> {
    let ancestors = store.ancestors_by_query(chunk)?;
    println!("{}", fmt.box_top(&chunk.source_path));
    println!("{}", fmt.breadcrumb_nav(&ancestors, &chunk.heading));
    println!();

    if !chunk.content.is_empty() {
        println!("{}", chunk.content);
        println!();
    }

    let subtree = store.subtree(chunk.id)?;
    for (child, _depth) in &subtree {
        if child.id == chunk.id {
            continue;
        }
        println!("{}", fmt.section_rule(&child.heading));
        if !child.content.is_empty() {
            println!();
            println!("{}", child.content);
        }
        println!();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

fn cmd_index(
    store: &Store,
    path: Option<&str>,
    grep: Option<&str>,
    fmt: &Formatter,
) -> Result<(), Error> {
    let chunks = store.all_chunks()?;

    let chunks: Vec<&store::Chunk> = chunks
        .iter()
        .filter(|c| path.is_none_or(|p| c.source_path.starts_with(p)))
        .collect();

    let id_to_chunk: HashMap<i64, &store::Chunk> = chunks.iter().map(|c| (c.id, *c)).collect();

    if let Some(keyword) = grep {
        return cmd_index_grep(&chunks, keyword, &id_to_chunk, fmt);
    }

    let mut children_map: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
    for c in &chunks {
        let parent = c.parent_id.filter(|pid| id_to_chunk.contains_key(pid));
        children_map.entry(parent).or_default().push(c.id);
    }

    let mut source_order: Vec<&str> = Vec::new();
    let mut roots_by_source: HashMap<&str, Vec<i64>> = HashMap::new();
    for &id in children_map.get(&None).unwrap_or(&Vec::new()) {
        let chunk = id_to_chunk[&id];
        if !roots_by_source.contains_key(chunk.source_path.as_str()) {
            source_order.push(&chunk.source_path);
        }
        roots_by_source
            .entry(&chunk.source_path)
            .or_default()
            .push(id);
    }

    let mut first = true;
    for source in &source_order {
        if !first {
            println!();
        }
        first = false;
        println!("{}", fmt.bold(source));

        let roots = &roots_by_source[source];
        for (i, &root_id) in roots.iter().enumerate() {
            let is_last = i == roots.len() - 1;
            render_tree(
                root_id,
                &id_to_chunk,
                &children_map,
                fmt,
                &mut Vec::new(),
                is_last,
            );
        }
    }

    Ok(())
}

fn cmd_index_grep(
    chunks: &[&store::Chunk],
    keyword: &str,
    id_to_chunk: &HashMap<i64, &store::Chunk>,
    fmt: &Formatter,
) -> Result<(), Error> {
    let kw = keyword.to_lowercase();

    // Group matching chunks by source file.
    let mut source_order: Vec<&str> = Vec::new();
    let mut groups: HashMap<&str, Vec<&store::Chunk>> = HashMap::new();
    for chunk in chunks {
        if !chunk.heading.to_lowercase().contains(&kw) {
            continue;
        }
        if !groups.contains_key(chunk.source_path.as_str()) {
            source_order.push(&chunk.source_path);
        }
        groups.entry(&chunk.source_path).or_default().push(chunk);
    }

    let mut first = true;
    for source in &source_order {
        if !first {
            println!();
        }
        first = false;
        println!("{}", fmt.bold(source));

        let group = &groups[source];
        for (i, chunk) in group.iter().enumerate() {
            let is_last = i == group.len() - 1;
            let prefix = fmt.tree_connector(is_last);
            let ancestors = store::ancestors_from_chunks(chunk, id_to_chunk);
            let crumb = fmt.breadcrumb(&ancestors, &chunk.heading);
            println!("{prefix}{crumb} {}", fmt.slug(&chunk.slug));
        }
    }
    Ok(())
}

fn render_tree(
    chunk_id: i64,
    id_to_chunk: &HashMap<i64, &store::Chunk>,
    children_map: &HashMap<Option<i64>, Vec<i64>>,
    fmt: &Formatter,
    depth_prefixes: &mut Vec<bool>,
    is_last: bool,
) {
    let chunk = id_to_chunk[&chunk_id];

    let mut prefix = String::new();
    for &has_more in depth_prefixes.iter() {
        prefix.push_str(&fmt.tree_indent(has_more));
    }
    prefix.push_str(&fmt.tree_connector(is_last));

    println!(
        "{prefix}{} {}",
        fmt.bold(&chunk.heading),
        fmt.slug(&chunk.slug)
    );

    let empty = Vec::new();
    let children = children_map.get(&Some(chunk_id)).unwrap_or(&empty);
    depth_prefixes.push(!is_last);
    for (i, &child_id) in children.iter().enumerate() {
        render_tree(
            child_id,
            id_to_chunk,
            children_map,
            fmt,
            depth_prefixes,
            i == children.len() - 1,
        );
    }
    depth_prefixes.pop();
}

// ---------------------------------------------------------------------------
// prime
// ---------------------------------------------------------------------------

fn cmd_prime(store: &Store, install: Option<&Platform>) -> Result<(), Error> {
    match install {
        Some(Platform::Claude) => cmd_prime_install_claude(store),
        None => {
            print!("{}", include_str!("prime.md"));
            Ok(())
        }
    }
}

fn cmd_prime_install_claude(store: &Store) -> Result<(), Error> {
    let claude_dir = store.project_root().join(".claude");
    fs::create_dir_all(&claude_dir)?;

    let settings_path = claude_dir.join("settings.local.json");
    let mut settings = read_json_object(&settings_path)?;

    let allow_entry = serde_json::Value::String("Bash(stacks:*)".to_string());
    let permissions = ensure_object(settings.entry("permissions"));
    let allow = ensure_array(permissions.entry("allow"));
    if !allow.contains(&allow_entry) {
        allow.push(allow_entry);
    }

    let hook_command = "stacks prime";
    let hooks = ensure_object(settings.entry("hooks"));
    let session_start = ensure_array(hooks.entry("SessionStart"));

    let already_installed = session_start.iter().any(|entry| {
        entry["hooks"]
            .as_array()
            .is_some_and(|h| h.iter().any(|hook| hook["command"] == hook_command))
    });

    if !already_installed {
        session_start.push(serde_json::json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": hook_command}]
        }));
    }

    let json = serde_json::to_string_pretty(&settings).unwrap();
    fs::write(&settings_path, json + "\n")?;

    println!("Installed stacks SessionStart hook in .claude/settings.local.json");
    Ok(())
}

/// Ensure a JSON entry is an object, replacing it if it has the wrong type.
fn ensure_object(
    entry: serde_json::map::Entry<'_>,
) -> &mut serde_json::Map<String, serde_json::Value> {
    let value = entry.or_insert_with(|| serde_json::json!({}));
    if !value.is_object() {
        *value = serde_json::json!({});
    }
    value.as_object_mut().unwrap()
}

/// Ensure a JSON entry is an array, replacing it if it has the wrong type.
fn ensure_array(entry: serde_json::map::Entry<'_>) -> &mut Vec<serde_json::Value> {
    let value = entry.or_insert_with(|| serde_json::json!([]));
    if !value.is_array() {
        *value = serde_json::json!([]);
    }
    value.as_array_mut().unwrap()
}

fn read_json_object(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
    if path.exists() {
        let content = fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        Ok(value.as_object().cloned().unwrap_or_default())
    } else {
        Ok(serde_json::Map::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_json(content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "stacks-test-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("unknown")
        ));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn read_json_object_missing_file() {
        let result = read_json_object(Path::new("/nonexistent/path.json")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn read_json_object_valid() {
        let path = write_temp_json(r#"{"key": "value"}"#);
        let result = read_json_object(&path).unwrap();
        fs::remove_file(&path).ok();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn read_json_object_malformed() {
        let path = write_temp_json("not json");
        let result = read_json_object(&path);
        fs::remove_file(&path).ok();
        assert!(result.is_err());
    }

    #[test]
    fn read_json_object_non_object() {
        let path = write_temp_json("[1, 2, 3]");
        let result = read_json_object(&path).unwrap();
        fs::remove_file(&path).ok();
        assert!(result.is_empty());
    }

    #[test]
    fn ensure_object_creates_missing() {
        let mut map = serde_json::Map::new();
        let obj = ensure_object(map.entry("new"));
        obj.insert("k".into(), serde_json::json!("v"));
        assert_eq!(map["new"]["k"], "v");
    }

    #[test]
    fn ensure_object_replaces_wrong_type() {
        let mut map = serde_json::Map::new();
        map.insert("bad".into(), serde_json::json!("string"));
        let obj = ensure_object(map.entry("bad"));
        assert!(obj.is_empty());
    }

    #[test]
    fn ensure_array_creates_missing() {
        let mut map = serde_json::Map::new();
        let arr = ensure_array(map.entry("new"));
        arr.push(serde_json::json!(1));
        assert_eq!(map["new"][0], 1);
    }

    #[test]
    fn ensure_array_replaces_wrong_type() {
        let mut map = serde_json::Map::new();
        map.insert("bad".into(), serde_json::json!({"not": "array"}));
        let arr = ensure_array(map.entry("bad"));
        assert!(arr.is_empty());
    }
}
