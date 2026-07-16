use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use owo_colors::OwoColorize;

use crate::build::Workspace;
use crate::core::{BuildEnvironment, Result};

#[derive(Debug, Clone)]
pub struct WatchOpts {
    pub modules: Vec<PathBuf>,
    pub classmap: PathBuf,
    pub debounce: u64,
    pub dev: bool,
    pub source_map: bool,
}

pub fn run(opts: WatchOpts) -> Result<()> {
    let env = Arc::new(BuildEnvironment::current(opts.dev, opts.source_map));
    let workspace = Workspace::from_modules(&opts.modules, &opts.classmap, env)?;

    let started = Instant::now();
    let outcome = workspace.build_all()?;
    let max_name = outcome.keys().map(|n| n.len()).max().unwrap_or(0);
    for (name, build) in &outcome {
        let n = build.transpiled.len() + build.copied.len();
        println!(
            "{} {:<max_name$} {}",
            "  -".dimmed(),
            name.cyan(),
            format!("{:>3} files", n).yellow()
        );
    }
    println!(
        "{} {}",
        "👁".green().bold(),
        format!("ready — watching for changes ({:.2?})", started.elapsed()).dimmed(),
    );

    workspace.watch(Duration::from_millis(opts.debounce))
}
