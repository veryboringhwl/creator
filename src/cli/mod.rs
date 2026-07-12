pub mod build;
pub mod classmap;
pub mod new;
pub mod release;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::core::Result;

#[derive(Parser)]
#[command(
    name = "creator",
    version,
    about = "Build tool for Spicetify v3 modules"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
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
        template: Option<crate::scaffold::ModuleTemplate>,

        #[arg(long)]
        biome: Option<bool>,

        #[arg(long, default_value_t = false)]
        force: bool,
    },

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
        debounce: u64,

        #[arg(long = "dev", default_value_t = false)]
        dev: bool,

        #[arg(long = "source-map", default_value_t = false)]
        source_map: bool,
    },

    Release {
        #[arg(value_name = "INPUT_DIRS")]
        inputs: Vec<PathBuf>,

        #[arg(long = "classmap-url")]
        classmap_url: Option<String>,

        #[arg(long = "output-dir", default_value = "dist")]
        output_dir: PathBuf,

        #[arg(long = "dev", default_value_t = false)]
        dev: bool,
    },

    ClassmapFetch {
        #[arg(long = "url")]
        url: Option<String>,

        #[arg(long = "output", default_value = "classmap.json")]
        output: PathBuf,

        #[arg(long = "modules-dir", default_value = "modules")]
        modules_dir: PathBuf,
    },
}

pub fn dispatch(cli: Cli) -> Result<()> {
    init_tracing();
    match cli.command {
        Command::New {
            name,
            dir,
            author,
            description,
            template,
            biome,
            force,
        } => new::run(crate::scaffold::CliNewOpts {
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
            source_map,
        } => build::run(crate::cli::build::BuildOpts {
            module,
            input_dir,
            output_dir,
            classmap,
            watch,
            debounce,
            dev,
            source_map,
        }),
        Command::Release {
            inputs,
            classmap_url,
            output_dir,
            dev,
        } => {
            let url = resolve_classmap_url(classmap_url)?;
            release::run(crate::cli::release::ReleaseOpts {
                inputs,
                classmap_url: url,
                output_dir,
                dev,
            })
        }
        Command::ClassmapFetch {
            url,
            output,
            modules_dir,
        } => {
            let cm_url = resolve_classmap_url(url)?;
            classmap::run(crate::cli::classmap::FetchOpts {
                url: cm_url,
                output,
                modules_dir,
            })
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

const CLASSMAP_URL_ENV: &str = "CREATOR_CLASSMAP_URL";
const CLASSMAP_URL_FILE: &str = "classmap.url";

pub fn resolve_classmap_url(cli_value: Option<String>) -> Result<String> {
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
        std::path::Path::new("scripts").join("classmap-info.ts"),
        std::path::Path::new("scripts").join("classmap-info.js"),
        PathBuf::from("classmap-info.ts"),
        PathBuf::from("classmap-info.js"),
    ];
    for script_path in &script_candidates {
        if script_path.exists() {
            let script = std::fs::read_to_string(script_path)
                .map_err(|source| crate::core::Error::io(script_path, source))?;
            if let Some(url) = extract_classmap_url_from_script(&script) {
                return Ok(url);
            }
        }
    }
    let file_path = std::path::Path::new(CLASSMAP_URL_FILE);
    if file_path.exists() {
        let url = std::fs::read_to_string(file_path)
            .map_err(|source| crate::core::Error::io(file_path, source))?;
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        return Err(crate::core::Error::Scaffold(format!(
            "{} is empty",
            file_path.display()
        )));
    }
    Err(crate::core::Error::Scaffold(format!(
        "No classmap URL found. Provide --classmap-url, set {CLASSMAP_URL_ENV}, or create a classmap-info script."
    )))
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
