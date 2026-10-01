use clap::{Parser, ValueEnum};
use dux::browse::{run_script, run_terminal, Browser};
use dux::render::{human, render_tree, thousands, View};
use dux::scan::{scan, Options};
use dux::tree::{by_extension, largest_files, pruned, SortBy};
use dux::parse_size;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

#[derive(Clone, Copy, ValueEnum)]
enum Sort {
    Size,
    Files,
    Name,
}

#[derive(Parser)]
#[command(version, about = "Parallel disk usage scanner")]
struct Cli {
    /// Directory or file to measure (default: current directory)
    path: Option<PathBuf>,
    /// Levels of the tree to print
    #[arg(short, long, default_value_t = 2)]
    depth: usize,
    /// Entries per directory
    #[arg(short = 'n', long, default_value_t = 10)]
    top: usize,
    #[arg(long, value_enum, default_value = "size")]
    sort: Sort,
    /// Hide entries smaller than this (500, 64k, 10M, 2G)
    #[arg(long, value_name = "SIZE")]
    min_size: Option<String>,
    /// Count allocated bytes (compressed or sparse files, disk blocks) instead of file length
    #[arg(long)]
    disk: bool,
    /// Follow symlinks and junctions (loops are detected)
    #[arg(short = 'L', long)]
    follow: bool,
    /// Count every hard link separately and skip the per-file check (faster on Windows)
    #[arg(long)]
    no_hard_links: bool,
    /// Worker threads (default: number of CPUs)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
    /// List the N largest files
    #[arg(long, value_name = "N")]
    files: Option<usize>,
    /// Show the largest extensions
    #[arg(long)]
    ext: bool,
    /// Plain ASCII bars
    #[arg(long)]
    ascii: bool,
    /// Print the tree as JSON
    #[arg(long)]
    json: bool,
    /// Browse the result interactively
    #[arg(short, long)]
    browse: bool,
    /// Replay keys without a terminal and print the final screen, for example "enter,down,s"
    #[arg(long, value_name = "KEYS", conflicts_with = "browse")]
    keys: Option<String>,
    /// Screen size for --keys
    #[arg(long, default_value = "100x24", value_name = "WxH")]
    screen: String,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dux: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    let min_size = cli.min_size.as_deref().map(parse_size).transpose()?.unwrap_or(0);
    let sort = match cli.sort {
        Sort::Size => SortBy::Size,
        Sort::Files => SortBy::Files,
        Sort::Name => SortBy::Name,
    };
    let opts = Options { follow: cli.follow, disk: cli.disk, hard_links: !cli.no_hard_links, threads: cli.threads };
    let started = Instant::now();
    let result = scan(&path, &opts).map_err(|e| format!("{}: {e}", path.display()))?;
    let elapsed = started.elapsed();

    if cli.json {
        let out = pruned(&result.root, cli.depth, cli.top, min_size, sort);
        println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
        return Ok(());
    }
    if let Some(keys) = &cli.keys {
        let (w, h) = cli.screen.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).ok_or("--screen must look like 100x24")?;
        print!("{}", run_script(Browser::new(result.root, sort, cli.ascii), keys, w, h)?);
        return Ok(());
    }
    if cli.browse {
        return run_terminal(Browser::new(result.root, sort, cli.ascii)).map_err(|e| e.to_string());
    }

    print!("{}", render_tree(&result.root, &View { depth: cli.depth, top: cli.top, sort, min_size, ascii: cli.ascii }));
    if let Some(n) = cli.files {
        println!("\nlargest files:");
        for (size, p) in largest_files(&result.root, n) {
            println!("{:>10}  {p}", human(size));
        }
    }
    if cli.ext {
        println!("\nlargest extensions:");
        for (e, size, count) in by_extension(&result.root).into_iter().take(cli.top) {
            println!("{:>10}  {:>10} files  {}", human(size), thousands(count), if e.is_empty() { "(none)".to_string() } else { format!(".{e}") });
        }
    }
    eprintln!("\nscanned {} files in {:.2} s", thousands(result.root.files), elapsed.as_secs_f64());
    if result.shared_links > 0 {
        eprintln!("{} hard-linked paths counted once", thousands(result.shared_links));
    }
    if result.root.errors > 0 {
        eprintln!("{} entries could not be read, for example:", thousands(result.root.errors));
        for e in result.errors.iter().take(3) {
            eprintln!("  {e}");
        }
    }
    for l in result.loops.iter().take(3) {
        eprintln!("symlink loop skipped: {l}");
    }
    Ok(())
}
