use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use minijinja::{Environment, context};

use crate::core::util::write_text;
use crate::core::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ModuleTemplate {
    CustomApp,
    Extension,
}

impl ModuleTemplate {
    pub const ALL: &'static [Self] = &[Self::CustomApp, Self::Extension];

    pub fn label(self) -> &'static str {
        match self {
            Self::CustomApp => "custom-app    (TSX + React, .tsx)",
            Self::Extension => "extension     (plain TypeScript, .ts)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliNewOpts {
    pub name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub template: Option<ModuleTemplate>,
    pub biome: Option<bool>,
    pub dir: Option<PathBuf>,
    pub force: bool,
}

#[derive(Debug, Clone)]
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
    let cwd = std::env::current_dir().map_err(|e| Error::Scaffold(e.to_string()))?;
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
    opts.name.is_some() && opts.author.is_some() && opts.template.is_some()
}

fn build_from_cli(opts: CliNewOpts) -> Result<ProjectOpts> {
    let name = opts
        .name
        .ok_or_else(|| Error::Scaffold("--name is required in non-interactive mode".into()))?;
    let author = opts.author.unwrap_or_else(guess_author);
    let template = opts
        .template
        .ok_or_else(|| Error::Scaffold("--template is required in non-interactive mode".into()))?;
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

    let name: String = dialoguer::Input::new()
        .with_prompt("Module name")
        .default(default_name.to_string())
        .interact_text()
        .map_err(|e| Error::Scaffold(e.to_string()))?;

    let description: String = dialoguer::Input::new()
        .with_prompt("Description")
        .default("A Spicetify v3 module".to_string())
        .allow_empty(true)
        .interact_text()
        .map_err(|e| Error::Scaffold(e.to_string()))?;

    let author: String = dialoguer::Input::new()
        .with_prompt("Author")
        .default(guess_author())
        .interact_text()
        .map_err(|e| Error::Scaffold(e.to_string()))?;

    let template_idx = dialoguer::Select::new()
        .with_prompt("Template")
        .items(ModuleTemplate::ALL.iter().map(|t| t.label()))
        .default(0)
        .interact()
        .map_err(|e| Error::Scaffold(e.to_string()))?;
    let template = ModuleTemplate::ALL[template_idx];

    let biome = if in_modules_repo {
        false
    } else {
        dialoguer::Confirm::new()
            .with_prompt("Include Biome config?")
            .default(true)
            .interact()
            .map_err(|e| Error::Scaffold(e.to_string()))?
    };

    let dir = if in_modules_repo {
        PathBuf::from("modules")
    } else {
        let input: String = dialoguer::Input::new()
            .with_prompt("Modules directory")
            .default("modules".to_string())
            .interact_text()
            .map_err(|e| Error::Scaffold(e.to_string()))?;
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
        return Err(Error::Scaffold(format!(
            "module directory already exists: {} (use --force to overwrite)",
            module_dir.display()
        )));
    }
    fs::create_dir_all(&module_dir).map_err(|source| Error::io(&module_dir, source))?;

    let files = render_module_files(opts);
    for file in &files {
        let dest = module_dir.join(&file.path);
        write_text(&dest, &file.contents)?;
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
        return Err(Error::Scaffold(format!(
            "project directory already exists: {} (use --force to overwrite)",
            root.display()
        )));
    }
    fs::create_dir_all(&root).map_err(|source| Error::io(&root, source))?;

    let project_files = render_project_files(opts);
    for file in &project_files {
        let dest = root.join(&file.path);
        write_text(&dest, &file.contents)?;
        println!("  + {}", dest.display());
    }
    let module_dir = root.join(&opts.dir).join(&opts.name);
    fs::create_dir_all(&module_dir).map_err(|source| Error::io(&module_dir, source))?;
    let module_files = render_module_files(opts);
    for file in &module_files {
        let dest = module_dir.join(&file.path);
        write_text(&dest, &file.contents)?;
        println!("  + {}", dest.display());
    }
    println!();
    println!("Project \"{}\" created!", opts.name);
    println!();
    println!("  cd {}", opts.name);
    println!("  deno task build");
    Ok(())
}

#[derive(Debug)]
struct RenderedFile {
    path: PathBuf,
    contents: String,
}

fn render_module_files(opts: &ProjectOpts) -> Vec<RenderedFile> {
    let env = create_env();
    let ctx = context! {
        MODULE_NAME => &opts.name,
        AUTHOR => &opts.author,
        DESCRIPTION => &opts.description,
    };
    module_template_files(opts.template)
        .into_iter()
        .map(|(path, src)| {
            let tmpl = env
                .template_from_str(src)
                .expect("module template compiles");
            let contents = tmpl.render(ctx.clone()).expect("module template renders");
            RenderedFile {
                path: PathBuf::from(path),
                contents,
            }
        })
        .collect()
}

fn render_project_files(opts: &ProjectOpts) -> Vec<RenderedFile> {
    let env = create_env();
    let ctx = context! {
        MODULE_NAME => &opts.name,
        AUTHOR => &opts.author,
        DESCRIPTION => &opts.description,
        CREATOR_VERSION => env!("CARGO_PKG_VERSION"),
        BIOME => opts.biome,
    };
    let mut files: Vec<(&'static str, String)> = project_template_files()
        .into_iter()
        .map(|(path, src)| {
            let tmpl = env
                .template_from_str(src)
                .expect("project template compiles");
            let contents = tmpl.render(ctx.clone()).expect("project template renders");
            (path, contents)
        })
        .collect();
    if opts.biome {
        let tmpl = env
            .template_from_str(PROJECT_BIOME_JSON)
            .expect("biome template compiles");
        let contents = tmpl.render(ctx.clone()).expect("biome template renders");
        files.push(("biome.json", contents));
    }
    files
        .into_iter()
        .map(|(path, contents)| RenderedFile {
            path: PathBuf::from(path),
            contents,
        })
        .collect()
}

fn create_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_name| minijinja::AutoEscape::None);
    env
}

fn module_template_files(template: ModuleTemplate) -> Vec<(&'static str, &'static str)> {
    match template {
        ModuleTemplate::CustomApp => vec![
            ("metadata.json", MODULE_CUSTOM_APP_METADATA),
            ("index.ts", MODULE_CUSTOM_APP_INDEX_TS),
            ("load.ts", MODULE_CUSTOM_APP_LOAD_TS),
            ("mixin.ts", MODULE_CUSTOM_APP_MIXIN_TS),
            ("index.css", MODULE_CUSTOM_APP_CSS),
        ],
        ModuleTemplate::Extension => vec![
            ("metadata.json", MODULE_EXTENSION_METADATA),
            ("index.ts", MODULE_EXTENSION_INDEX_TS),
            ("load.ts", MODULE_EXTENSION_LOAD_TS),
            ("mixin.ts", MODULE_EXTENSION_MIXIN_TS),
            ("index.css", MODULE_EXTENSION_CSS),
        ],
    }
}

fn project_template_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("deno.json", PROJECT_DENO_JSON),
        ("classmap.json", PROJECT_CLASSMAP_JSON),
        ("vault.json", PROJECT_VAULT_JSON),
        (".gitignore", PROJECT_GITIGNORE),
        (".editorconfig", PROJECT_EDITORCONFIG),
        ("scripts/build.ts", PROJECT_BUILD_TS),
        ("scripts/watch.ts", PROJECT_WATCH_TS),
        ("scripts/enable.ts", PROJECT_ENABLE_TS),
        ("scripts/release.ts", PROJECT_RELEASE_TS),
    ]
}

const MODULE_CUSTOM_APP_METADATA: &str = include_str!("../../templates/modules/app/metadata.json");
const MODULE_CUSTOM_APP_INDEX_TS: &str = include_str!("../../templates/modules/app/index.ts");
const MODULE_CUSTOM_APP_LOAD_TS: &str = include_str!("../../templates/modules/app/load.ts");
const MODULE_CUSTOM_APP_MIXIN_TS: &str = include_str!("../../templates/modules/app/mixin.ts");
const MODULE_CUSTOM_APP_CSS: &str = include_str!("../../templates/modules/app/index.css");

const MODULE_EXTENSION_METADATA: &str =
    include_str!("../../templates/modules/extension/metadata.json");
const MODULE_EXTENSION_INDEX_TS: &str = include_str!("../../templates/modules/extension/index.ts");
const MODULE_EXTENSION_LOAD_TS: &str = include_str!("../../templates/modules/extension/load.ts");
const MODULE_EXTENSION_MIXIN_TS: &str = include_str!("../../templates/modules/extension/mixin.ts");
const MODULE_EXTENSION_CSS: &str = include_str!("../../templates/modules/extension/index.css");

const PROJECT_DENO_JSON: &str = include_str!("../../templates/deno.json");
const PROJECT_CLASSMAP_JSON: &str = include_str!("../../templates/classmap.json");
const PROJECT_VAULT_JSON: &str = include_str!("../../templates/vault.json");
const PROJECT_GITIGNORE: &str = include_str!("../../templates/.gitignore");
const PROJECT_EDITORCONFIG: &str = include_str!("../../templates/.editorconfig");
const PROJECT_BIOME_JSON: &str = include_str!("../../templates/biome.json");
const PROJECT_BUILD_TS: &str = include_str!("../../templates/scripts/build.ts");
const PROJECT_WATCH_TS: &str = include_str!("../../templates/scripts/watch.ts");
const PROJECT_ENABLE_TS: &str = include_str!("../../templates/scripts/enable.ts");
const PROJECT_RELEASE_TS: &str = include_str!("../../templates/scripts/release.ts");
