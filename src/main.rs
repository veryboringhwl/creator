mod builder;
mod classmap;
mod release;
mod scaffold;
mod timestamp;
mod transpile;
mod util;
mod watch;

use std::{
    fs, path::{Path, PathBuf}, time::Instant
};

use anyhow::{Context, Result};
use builder::{BuildOpts, Builder, BuilderOpts, Metadata};
use clap::{Parser, Subcommand};
use classmap::{discover_module_dirs, fetch_classmap_info, gen_classmap_dts};
use scaffold::CliNewOpts;
use transpile::Transpiler;
use util::{read_json, write_text};

const CLASSMAP_URL_ENV: &str = "CREATOR_CLASSMAP_URL";
const CLASSMAP_URL_FILE: &str = "classmap.url";

#[derive(Parser)]
#[command(
    name = "creator",
    version,
    about = "Build tool for Spicetify v3 modules"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    New {
        #[arg(long)]
        name: Option<String>,

        #[arg(long)]
        dir: Option<PathBuf>,

        #[arg(long)]
        author: Option<String>,

        #[arg(long)]
        description: Option<String>,

        #[arg(long)]
        template: Option<scaffold::ModuleTemplate>,

        #[arg(long)]
        biome: Option<bool>,

        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// Build a single module
    Build {
        #[arg(long)]
        module: Option<String>,

        #[arg(short = 'i', long = "input-dir")]
        input_dir: PathBuf,

        #[arg(short = 'o', long = "output-dir")]
        output_dir: PathBuf,

        #[arg(short = 'c', long = "classmap", default_value = "classmap.json")]
        classmap: PathBuf,

        #[arg(short = 'w', long = "watch", default_value_t = false)]
        watch: bool,

        #[arg(long = "debounce", default_value_t = 1000)]
        debounce: i64,

        #[arg(long = "dev", default_value_t = false)]
        dev: bool,
    },

    /// Build all modules into dist/ for release (zips + manifest)
    Release {
        #[arg(value_name = "INPUT_DIRS")]
        inputs: Vec<PathBuf>,

        #[arg(long = "classmap-url")]
        classmap_url: Option<String>,

        #[arg(long = "output-dir", default_value = "dist")]
        output_dir: PathBuf,
    },

    /// Fetch classmap and generate .d.ts files
    ClassmapFetch {
        #[arg(long = "url")]
        url: Option<String>,

        #[arg(long = "output", default_value = "classmap.json")]
        output: PathBuf,

        #[arg(long = "modules-dir", default_value = "modules")]
        modules_dir: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let started = Instant::now();

    let result = match cli.command {
        Command::New {
            name,
            dir,
            author,
            description,
            template,
            biome,
            force,
        } => scaffold::run_new(CliNewOpts {
            name,
            author,
            description,
            template,
            biome,
            dir,
            force,
        }),

        Command::Build {
            module,
            input_dir,
            output_dir,
            classmap,
            watch,
            debounce,
            dev,
        } => run_build(
            module, input_dir, output_dir, classmap, watch, debounce, dev,
        ),

        Command::Release {
            inputs,
            classmap_url,
            output_dir,
        } => {
            let url = resolve_classmap_url(classmap_url)?;
            release::run_release(release::ReleaseOpts {
                inputs,
                classmap_url: url,
                output_dir,
            })
        }

        Command::ClassmapFetch {
            url,
            output,
            modules_dir,
        } => {
            let cm_url = resolve_classmap_url(url)?;
            run_classmap_fetch(&cm_url, &output, &modules_dir)
        }
    };

    let elapsed = started.elapsed();
    eprintln!("Finished in {elapsed:.2?}");

    result
}

fn run_build(
    module: Option<String>,
    input_dir: PathBuf,
    output_dir: PathBuf,
    classmap: PathBuf,
    watch: bool,
    debounce: i64,
    dev: bool,
) -> Result<()> {
    let metadata_path = input_dir.join("metadata.json");
    let metadata: Metadata = read_json(&metadata_path)
        .with_context(|| format!("Failed to read {}", metadata_path.display()))?;

    let identifier = match module {
        Some(name) => name,
        None => metadata
            .name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--module is required when metadata.name is missing"))?,
    };

    let mapping =
        read_json(&classmap).with_context(|| format!("Failed to read {}", classmap.display()))?;

    let builder = Builder::new(
        Transpiler::new(mapping, dev),
        BuilderOpts {
            metadata,
            identifier,
            input_dir,
            output_dir,
        },
    )?;

    builder.build(BuildOpts {
        js: true,
        css: true,
        unknown: false,
    })?;

    if watch {
        crate::watch::watch(&builder, debounce)?;
    }

    Ok(())
}

fn run_classmap_fetch(url: &str, output: &Path, modules_dir: &Path) -> Result<()> {
    let info = fetch_classmap_info(url)?;
    let json = serde_json::to_string(&info.mapping).context("Failed to serialize classmap")?;

    write_text(output, &json)?;
    println!("Saved classmap to {}", output.display());

    let dts = gen_classmap_dts(&info.mapping);
    let modules = discover_module_dirs(modules_dir)?;
    for module in modules {
        let dts_path = module.join("classmap.d.ts");
        write_text(&dts_path, &dts)?;
    }

    Ok(())
}

fn resolve_classmap_url(cli_value: Option<String>) -> Result<String> {
    if let Some(url) = cli_value {
        return Ok(url);
    }

    if let Ok(url) = std::env::var(CLASSMAP_URL_ENV) {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let script_candidates = [
        Path::new("scripts").join("classmap-info.ts"),
        Path::new("scripts").join("classmap-info.js"),
        PathBuf::from("classmap-info.ts"),
        PathBuf::from("classmap-info.js"),
    ];

    for script_path in &script_candidates {
        if script_path.exists() {
            let script = fs::read_to_string(script_path)
                .with_context(|| format!("Failed to read {}", script_path.display()))?;
            if let Some(url) = extract_classmap_url_from_script(&script) {
                return Ok(url);
            }
        }
    }

    let file_path = Path::new(CLASSMAP_URL_FILE);
    if file_path.exists() {
        let url = fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        return Err(anyhow::anyhow!("{} is empty", file_path.display()));
    }

    Err(anyhow::anyhow!(
        "No classmap URL found. Provide --classmap-url, set {CLASSMAP_URL_ENV}, or create a classmap-info script."
    ))
}

fn extract_classmap_url_from_script(script: &str) -> Option<String> {
    script
        .split(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '`')
        .find(|token| {
            token.starts_with("https://raw.githubusercontent.com/")
                && token.contains("/classmaps/")
                && token.ends_with("/classmap.json")
        })
        .map(ToString::to_string)
}
