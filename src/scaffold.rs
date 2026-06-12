use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use dialoguer::{Confirm, Input, Select};

use crate::util::write_text;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ModuleTemplate {
    #[value(name = "custom-app")]
    CustomApp,
    #[value(name = "extension")]
    Extension,
}

impl ModuleTemplate {
    const ALL: &[Self] = &[Self::CustomApp, Self::Extension];

    fn label(&self) -> &'static str {
        match self {
            Self::CustomApp => "custom-app    (TSX + React, .tsx)",
            Self::Extension => "extension     (plain TypeScript, .ts)",
        }
    }
}

#[derive(Debug)]
pub struct CliNewOpts {
    pub name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub template: Option<ModuleTemplate>,
    pub biome: Option<bool>,
    pub dir: Option<PathBuf>,
    pub force: bool,
}

#[derive(Debug)]
struct ProjectOpts {
    name: String,
    author: String,
    description: String,
    template: ModuleTemplate,
    biome: bool,
    dir: PathBuf,
    force: bool,
}

pub fn run_new(opts: CliNewOpts) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let in_modules_repo = is_modules_repo(&cwd);

    let project = if has_required_cli_args(&opts) {
        build_from_cli(opts)?
    } else {
        run_wizard(&cwd, in_modules_repo)?
    };

    if in_modules_repo {
        scaffold_module(&project)
    } else {
        scaffold_project(&project)
    }
}

fn is_modules_repo(cwd: &Path) -> bool {
    cwd.join("deno.json").exists() && cwd.join("modules").is_dir()
        || cwd.join("modules").join("deno.json").exists()
}

fn has_required_cli_args(opts: &CliNewOpts) -> bool {
    opts.name.is_some() && opts.author.is_some() && opts.template.is_some() && opts.biome.is_some()
}

fn build_from_cli(opts: CliNewOpts) -> Result<ProjectOpts> {
    let name = opts
        .name
        .ok_or_else(|| anyhow::anyhow!("--name is required in non-interactive mode"))?;
    let author = opts.author.unwrap_or_else(guess_author);
    let template = opts
        .template
        .ok_or_else(|| anyhow::anyhow!("--template is required in non-interactive mode"))?;
    let biome = opts.biome.unwrap_or(true);
    let dir = opts.dir.unwrap_or_else(|| PathBuf::from("modules"));
    let description = opts.description.unwrap_or_default();

    Ok(ProjectOpts {
        name,
        author,
        description,
        template,
        biome,
        dir,
        force: opts.force,
    })
}

fn run_wizard(cwd: &Path, in_modules_repo: bool) -> Result<ProjectOpts> {
    println!();
    println!("  create-spicetify-module");
    println!();

    let default_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-module");

    let name: String = Input::new()
        .with_prompt("Module name")
        .default(default_name.to_string())
        .interact_text()?;

    let description: String = Input::new()
        .with_prompt("Description")
        .default("A Spicetify v3 module".to_string())
        .allow_empty(true)
        .interact_text()?;

    let author: String = Input::new()
        .with_prompt("Author")
        .default(guess_author())
        .interact_text()?;

    let template_idx = Select::new()
        .with_prompt("Template")
        .items(
            ModuleTemplate::ALL
                .iter()
                .map(|t| t.label())
                .collect::<Vec<_>>(),
        )
        .default(0)
        .interact()?;
    let template = ModuleTemplate::ALL[template_idx];

    let biome = if in_modules_repo {
        false
    } else {
        Confirm::new()
            .with_prompt("Include Biome config?")
            .default(true)
            .interact()?
    };

    let dir = if in_modules_repo {
        PathBuf::from("modules")
    } else {
        let input: String = Input::new()
            .with_prompt("Modules directory")
            .default("modules".to_string())
            .interact_text()?;
        PathBuf::from(input)
    };

    Ok(ProjectOpts {
        name,
        author,
        description,
        template,
        biome,
        dir,
        force: false,
    })
}

fn guess_author() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "author".to_string())
}

fn scaffold_module(opts: &ProjectOpts) -> Result<()> {
    let module_dir = opts.dir.join(&opts.name);

    if module_dir.exists() && !opts.force {
        return Err(anyhow::anyhow!(
            "Module directory already exists: {} (use --force to overwrite)",
            module_dir.display()
        ));
    }

    fs::create_dir_all(&module_dir)
        .with_context(|| format!("Failed to create {}", module_dir.display()))?;

    let files = render_module_files(opts.template, &opts.name, &opts.author, &opts.description);

    for (rel, contents) in &files {
        let dest = module_dir.join(rel);
        write_text(&dest, contents)?;
        println!("  + {}", dest.display());
    }

    println!();
    println!(
        "Module \"{}\" created ({})",
        opts.name,
        opts.template.label()
    );

    Ok(())
}

fn scaffold_project(opts: &ProjectOpts) -> Result<()> {
    let root = PathBuf::from(&opts.name);

    if root.exists() && !opts.force {
        return Err(anyhow::anyhow!(
            "Project directory already exists: {} (use --force to overwrite)",
            root.display()
        ));
    }

    fs::create_dir_all(&root).with_context(|| format!("Failed to create {}", root.display()))?;

    // Root-level config files
    let project_files = render_project_files(opts);
    for (rel, contents) in project_files {
        let dest = root.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).ok();
        }
        write_text(&dest, &contents)?;
        println!("  + {}", dest.display());
    }

    // Module inside the project
    let module_dir = root.join(&opts.dir).join(&opts.name);
    fs::create_dir_all(&module_dir)
        .with_context(|| format!("Failed to create {}", module_dir.display()))?;

    let module_files =
        render_module_files(opts.template, &opts.name, &opts.author, &opts.description);
    for (rel, contents) in &module_files {
        let dest = module_dir.join(rel);
        write_text(&dest, contents)?;
        println!("  + {}", dest.display());
    }

    println!();
    println!("Project \"{}\" created!", opts.name);
    println!();
    println!("  cd {}", opts.name);
    println!("  deno task build");

    Ok(())
}

fn apply_tokens(content: &str, tokens: &[(&str, &str)]) -> String {
    let mut out = content.to_string();
    for (key, val) in tokens {
        out = out.replace(&format!("{{{{{key}}}}}"), val);
    }
    out
}

fn render_module_files(
    template: ModuleTemplate,
    name: &str,
    author: &str,
    description: &str,
) -> Vec<(PathBuf, String)> {
    let base = match template {
        ModuleTemplate::CustomApp => module_files_custom_app(),
        ModuleTemplate::Extension => module_files_extension(),
    };

    let tokens: &[(&str, &str)] = &[
        ("MODULE_NAME", name),
        ("AUTHOR", author),
        ("DESCRIPTION", description),
    ];

    base.into_iter()
        .map(|(rel, raw)| (rel, apply_tokens(raw, tokens)))
        .collect()
}

fn render_project_files(opts: &ProjectOpts) -> Vec<(PathBuf, String)> {
    let mut files = vec![
        (PathBuf::from("deno.json"), PROJECT_DENO_JSON.to_string()),
        (
            PathBuf::from("classmap.json"),
            PROJECT_CLASSMAP_JSON.to_string(),
        ),
        (PathBuf::from("vault.json"), PROJECT_VAULT_JSON.to_string()),
        (PathBuf::from(".gitignore"), PROJECT_GITIGNORE.to_string()),
        (
            PathBuf::from(".editorconfig"),
            PROJECT_EDITORCONFIG.to_string(),
        ),
        (
            PathBuf::from("scripts/build-dev.ps1"),
            PROJECT_BUILD_DEV_PS1.to_string(),
        ),
        (
            PathBuf::from("scripts/watch-dev.ps1"),
            PROJECT_WATCH_DEV_PS1.to_string(),
        ),
        (
            PathBuf::from("scripts/enable-dev.ps1"),
            PROJECT_ENABLE_DEV_PS1.to_string(),
        ),
        (
            PathBuf::from("scripts/build-dev.sh"),
            PROJECT_BUILD_DEV_SH.to_string(),
        ),
        (
            PathBuf::from("scripts/watch-dev.sh"),
            PROJECT_WATCH_DEV_SH.to_string(),
        ),
        (
            PathBuf::from("scripts/enable-dev.sh"),
            PROJECT_ENABLE_DEV_SH.to_string(),
        ),
        (
            PathBuf::from("scripts/build-local.ts"),
            PROJECT_BUILD_LOCAL_TS.to_string(),
        ),
        (
            PathBuf::from("scripts/build-shared.ts"),
            PROJECT_BUILD_SHARED_TS.to_string(),
        ),
        (
            PathBuf::from("scripts/cron.ts"),
            PROJECT_CRON_TS.to_string(),
        ),
    ];

    if opts.biome {
        files.push((PathBuf::from("biome.json"), PROJECT_BIOME_JSON.to_string()));
    }

    files
}

fn module_files_custom_app() -> Vec<(PathBuf, &'static str)> {
    vec![
        (PathBuf::from("metadata.json"), MODULE_CUSTOM_APP_METADATA),
        (PathBuf::from("index.ts"), MODULE_CUSTOM_APP_INDEX_TS),
        (PathBuf::from("load.ts"), MODULE_CUSTOM_APP_LOAD_TS),
        (PathBuf::from("mixin.ts"), MODULE_CUSTOM_APP_MIXIN_TS),
        (PathBuf::from("index.css"), MODULE_CUSTOM_APP_CSS),
    ]
}

fn module_files_extension() -> Vec<(PathBuf, &'static str)> {
    vec![
        (PathBuf::from("metadata.json"), MODULE_EXTENSION_METADATA),
        (PathBuf::from("index.ts"), MODULE_EXTENSION_INDEX_TS),
        (PathBuf::from("load.ts"), MODULE_EXTENSION_LOAD_TS),
        (PathBuf::from("mixin.ts"), MODULE_EXTENSION_MIXIN_TS),
        (PathBuf::from("index.css"), MODULE_EXTENSION_CSS),
    ]
}

const MODULE_CUSTOM_APP_METADATA: &str = include_str!("../templates/modules/app/metadata.json");
const MODULE_CUSTOM_APP_INDEX_TS: &str = include_str!("../templates/modules/app/index.ts");
const MODULE_CUSTOM_APP_LOAD_TS: &str = include_str!("../templates/modules/app/load.ts");
const MODULE_CUSTOM_APP_MIXIN_TS: &str = include_str!("../templates/modules/app/mixin.ts");
const MODULE_CUSTOM_APP_CSS: &str = include_str!("../templates/modules/app/index.css");

const MODULE_EXTENSION_METADATA: &str =
    include_str!("../templates/modules/extension/metadata.json");
const MODULE_EXTENSION_INDEX_TS: &str = include_str!("../templates/modules/extension/index.ts");
const MODULE_EXTENSION_LOAD_TS: &str = include_str!("../templates/modules/extension/load.ts");
const MODULE_EXTENSION_MIXIN_TS: &str = include_str!("../templates/modules/extension/mixin.ts");
const MODULE_EXTENSION_CSS: &str = include_str!("../templates/modules/extension/index.css");

const PROJECT_DENO_JSON: &str = include_str!("../templates/deno.json");
const PROJECT_CLASSMAP_JSON: &str = include_str!("../templates/classmap.json");
const PROJECT_VAULT_JSON: &str = include_str!("../templates/vault.json");
const PROJECT_GITIGNORE: &str = include_str!("../templates/.gitignore");
const PROJECT_EDITORCONFIG: &str = include_str!("../templates/.editorconfig");

const PROJECT_BUILD_DEV_PS1: &str = include_str!("../templates/scripts/build-dev.ps1");
const PROJECT_WATCH_DEV_PS1: &str = include_str!("../templates/scripts/watch-dev.ps1");
const PROJECT_ENABLE_DEV_PS1: &str = include_str!("../templates/scripts/enable-dev.ps1");
const PROJECT_BUILD_DEV_SH: &str = include_str!("../templates/scripts/build-dev.sh");
const PROJECT_WATCH_DEV_SH: &str = include_str!("../templates/scripts/watch-dev.sh");
const PROJECT_ENABLE_DEV_SH: &str = include_str!("../templates/scripts/enable-dev.sh");
const PROJECT_BUILD_LOCAL_TS: &str = include_str!("../templates/scripts/build-local.ts");
const PROJECT_BUILD_SHARED_TS: &str = include_str!("../templates/scripts/build-shared.ts");
const PROJECT_CRON_TS: &str = include_str!("../templates/scripts/cron.ts");
const PROJECT_BIOME_JSON: &str = include_str!("../templates/biome.json");
