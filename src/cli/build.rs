use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use owo_colors::OwoColorize;

use crate::build::{Builder, DriverOptions, WatchOptions, Workspace, watch};
use crate::core::{BuildEnvironment, Error, Metadata, Plan, Result, classmap, util};

#[derive(Debug, Clone)]
pub struct BuildOpts {
    pub module: Option<String>,
    pub input_dir: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub modules: Option<Vec<PathBuf>>,
    pub classmap: PathBuf,
    pub watch: bool,
    pub debounce: u64,
    pub dev: bool,
    pub source_map: bool,
}

pub fn run(opts: BuildOpts) -> Result<()> {
    if let Some(ref modules) = opts.modules {
        run_workspace(modules, &opts)
    } else {
        run_single(&opts)
    }
}

fn run_workspace(module_dirs: &[PathBuf], opts: &BuildOpts) -> Result<()> {
    let env = Arc::new(BuildEnvironment::current(opts.dev, opts.source_map));
    let workspace = Workspace::from_modules(module_dirs, &opts.classmap, env)?;

    let started = Instant::now();
    let outcome = workspace.build_all()?;
    let max_name = outcome.keys().map(|n| n.len()).max().unwrap_or(0);
    let total: usize = outcome
        .values()
        .map(|b| b.transpiled.len() + b.copied.len())
        .sum();
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
        "{}  {:<max_name$} {}",
        "  ─".dimmed(),
        "",
        format!("{:>3} files", total).yellow().bold(),
    );
    println!(
        "{} {} modules {}",
        "✔".green().bold(),
        outcome.len(),
        format!("({:.2?})", started.elapsed()).dimmed(),
    );

    if opts.watch {
        workspace.watch(Duration::from_millis(opts.debounce))?;
    }
    Ok(())
}

fn run_single(opts: &BuildOpts) -> Result<()> {
    let input_dir = opts
        .input_dir
        .as_ref()
        .ok_or_else(|| Error::Plan("--input-dir or --modules-dir is required".into()))?;
    let output_dir = opts.output_dir.as_ref().ok_or_else(|| {
        Error::Plan("--output-dir is required (or use --modules-dir for workspace mode)".into())
    })?;

    let metadata_path = input_dir.join("metadata.json");
    let metadata: Metadata = util::read_json(&metadata_path)?;

    let identifier = opts.module.clone().unwrap_or_else(|| metadata.name.clone());

    let mapping: classmap::Mapping = util::read_json(&opts.classmap)?;

    let env = Arc::new(BuildEnvironment::current(opts.dev, opts.source_map));
    let plan = Plan {
        metadata,
        source_dir: input_dir.clone(),
        output_dir: output_dir.clone(),
        classmap: Arc::new(mapping),
        env,
    };

    let builder = Builder::with_options(plan, DriverOptions::default())?;
    let started = Instant::now();
    let outcome = builder
        .run()
        .map_err(|e| Error::Release(format!("build {identifier}: {e:#}")))?;
    println!(
        "{} {} {}",
        "✔".green().bold(),
        identifier.cyan(),
        format!(
            "{} files ({:.2?})",
            outcome.transpiled.len() + outcome.copied.len(),
            started.elapsed()
        )
        .dimmed(),
    );

    if opts.watch {
        watch(
            &builder,
            WatchOptions {
                debounce: Duration::from_millis(opts.debounce),
                module: Some(identifier),
            },
        )?;
    }
    Ok(())
}
