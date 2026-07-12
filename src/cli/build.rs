use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::build::{Builder, DriverOptions, WatchOptions, watch};
use crate::core::{BuildEnvironment, Error, Metadata, Plan, Result, classmap, util};

#[derive(Debug, Clone)]
pub struct BuildOpts {
    pub module: Option<String>,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub classmap: PathBuf,
    pub watch: bool,
    pub debounce: u64,
    pub dev: bool,
    pub source_map: bool,
}

pub fn run(opts: BuildOpts) -> Result<()> {
    let metadata_path = opts.input_dir.join("metadata.json");
    let metadata: Metadata = util::read_json(&metadata_path)?;

    let identifier = opts.module.unwrap_or_else(|| metadata.name.clone());

    let mapping: classmap::Mapping = util::read_json(&opts.classmap)?;

    let env = Arc::new(BuildEnvironment::current(opts.dev, opts.source_map));
    let plan = Plan {
        metadata,
        source_dir: opts.input_dir.clone(),
        output_dir: opts.output_dir.clone(),
        classmap: Arc::new(mapping),
        env,
    };

    let builder = Builder::with_options(plan, DriverOptions::default())?;
    let started = Instant::now();
    let outcome = builder
        .run()
        .map_err(|e| Error::Release(format!("build {identifier}: {e:#}")))?;
    tracing::info!(
        identifier,
        elapsed_ms = started.elapsed().as_millis() as u64,
        transpiled = outcome.transpiled.len(),
        "build complete"
    );
    println!("{} finished in {:.2?}", identifier, started.elapsed());

    if opts.watch {
        watch(
            &builder,
            WatchOptions {
                debounce: Duration::from_millis(opts.debounce),
            },
        )?;
    }
    Ok(())
}
