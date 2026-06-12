use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::css::CssTranspiler;
use crate::js::JsTranspiler;
use crate::util::{ensure_parent, normalize_slashes};

#[derive(Clone, Debug, Deserialize)]
pub struct MetadataEntries {
    pub js: Option<String>,
    pub css: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Metadata {
    pub name: Option<String>,
    #[allow(dead_code)]
    pub version: Option<String>,
    pub entries: MetadataEntries,
    #[serde(default)]
    pub dependencies: MetadataDependencies,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum MetadataDependencies {
    List(Vec<String>),
    Map(HashMap<String, Value>),
}

impl Default for MetadataDependencies {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

impl MetadataDependencies {
    fn dependency_ids(&self) -> Vec<&str> {
        match self {
            MetadataDependencies::List(list) => list.iter().map(String::as_str).collect(),
            MetadataDependencies::Map(map) => map.keys().map(String::as_str).collect(),
        }
    }
}

pub struct BuilderOpts {
    pub metadata: Metadata,
    pub identifier: String,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
}

#[derive(Default, Clone, Copy)]
pub struct BuildOpts {
    pub js: bool,
    pub css: bool,
    pub unknown: bool,
}

pub struct Builder {
    scripts_input: Option<Vec<PathBuf>>,
    css_input: Option<CssInput>,
    pub identifier: String,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    js_transpiler: JsTranspiler,
    css_transpiler: CssTranspiler,
    pub dev: bool,
}

pub enum CssInput {
    Scss(PathBuf),
    Css(PathBuf),
}

impl Builder {
    pub fn new(
        js_transpiler: JsTranspiler,
        css_transpiler: CssTranspiler,
        dev: bool,
        opts: BuilderOpts,
    ) -> Result<Self> {
        let scripts_input = if opts.metadata.entries.js.is_some() {
            Some(collect_js_inputs(&opts.input_dir))
        } else {
            None
        };

        let css_input = if let Some(css) = &opts.metadata.entries.css {
            let scss_path = if css.ends_with(".css") {
                let mut stem = css.clone();
                stem.truncate(css.len() - 4);
                stem.push_str(".scss");
                opts.input_dir.join(stem)
            } else {
                opts.input_dir.join(css)
            };

            if scss_path.exists() {
                Some(CssInput::Scss(scss_path))
            } else {
                let css_path = opts.input_dir.join(css);
                if css_path.exists() {
                    Some(CssInput::Css(css_path))
                } else {
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            scripts_input,
            css_input,
            identifier: opts.identifier,
            input_dir: opts.input_dir,
            output_dir: opts.output_dir,
            js_transpiler,
            css_transpiler,
            dev,
        })
    }

    pub fn build(&self, opts: BuildOpts) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut scripts_input = Vec::new();
        let mut unknown_files = Vec::new();

        for entry in WalkDir::new(&self.input_dir) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let rel = self.get_relative_path(path);
            let file_type = parse_file_type(path, &rel);
            match file_type {
                FileType::ToJS => scripts_input.push(path.to_path_buf()),
                FileType::Unknown => unknown_files.push(rel),
                _ => {}
            }
        }

        scripts_input.sort();
        unknown_files.sort();

        let js_should_run = opts.js && self.scripts_input.is_some();
        let css_should_run = opts.css && self.css_input.is_some();
        let copy_should_run = opts.unknown && !unknown_files.is_empty();
        let did_build_js = js_should_run;
        let did_work = js_should_run || css_should_run || copy_should_run;

        let (js_result, css_result) = rayon::join(
            || {
                if js_should_run {
                    self.js(&scripts_input, now)
                } else {
                    Ok(())
                }
            },
            || {
                if css_should_run {
                    self.css(self.css_input.as_ref().unwrap())
                } else {
                    Ok(())
                }
            },
        );
        js_result?;
        css_result?;

        if copy_should_run {
            unknown_files
                .par_iter()
                .try_for_each(|rel| self.copy_file(rel))?;
        }

        if did_work {
            let timestamp = self.output_dir.join("timestamp");
            if self.dev {
                ensure_parent(&timestamp)?;
                fs::write(&timestamp, format!("{now}")).with_context(|| {
                    format!("Failed to write timestamp: {}", timestamp.display())
                })?;
            } else if let Err(err) = fs::remove_file(&timestamp)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err).with_context(|| {
                    format!("Failed to remove timestamp: {}", timestamp.display())
                });
            }
        }

        if self.dev && did_build_js {
            self.refresh_dev_dependents()?;
        }

        Ok(())
    }

    pub fn get_relative_path(&self, abs: &Path) -> PathBuf {
        abs.strip_prefix(&self.input_dir)
            .unwrap_or(abs)
            .to_path_buf()
    }

    pub fn get_input_path(&self, rel: &Path) -> PathBuf {
        self.input_dir.join(rel)
    }

    pub fn get_output_path(&self, rel: &Path) -> PathBuf {
        self.output_dir.join(rel)
    }

    fn js(&self, inputs: &[PathBuf], timestamp: u64) -> Result<()> {
        inputs.par_iter().try_for_each(|input| {
            let rel = self.get_relative_path(input);
            let mut rel_js = rel.clone();
            rel_js.set_extension("js");
            let output = self.get_output_path(&rel_js);

            if !needs_rebuild(input, &output)? {
                return Ok(());
            }

            let rel_js_str = normalize_slashes(&rel_js);
            let filepath = format!("/modules/{}/{}", self.identifier, rel_js_str);
            self.js_transpiler
                .transpile(input, &output, &filepath, timestamp)
        })
    }

    fn css(&self, input: &CssInput) -> Result<()> {
        match input {
            CssInput::Scss(path) => {
                let rel = self.get_relative_path(path);
                let mut rel_css = rel.clone();
                rel_css.set_extension("css");
                let output = self.get_output_path(&rel_css);

                if !needs_rebuild(path, &output)? {
                    return Ok(());
                }

                self.css_transpiler.transpile_scss(
                    path,
                    &output,
                    self.scripts_input.as_deref().unwrap_or(&[]),
                )?;
            }
            CssInput::Css(path) => {
                let rel = self.get_relative_path(path);
                let output = self.get_output_path(&rel);

                if !needs_rebuild(path, &output)? {
                    return Ok(());
                }

                self.css_transpiler.transpile_css(path, &output)?;
            }
        }
        Ok(())
    }

    pub fn copy_file(&self, rel: &Path) -> Result<()> {
        let input = self.get_input_path(rel);
        let output = self.get_output_path(rel);
        ensure_parent(&output)?;
        fs::copy(&input, &output).with_context(|| {
            format!("Failed to copy {} to {}", input.display(), output.display())
        })?;
        Ok(())
    }

    fn refresh_dev_dependents(&self) -> Result<()> {
        let modules_root = match self.infer_modules_root() {
            Some(path) => path,
            None => return Ok(()),
        };

        let dependents = self.discover_transitive_dependents(&modules_root)?;
        if dependents.is_empty() {
            return Ok(());
        }

        println!(
            "Refreshing {} dependent module(s) after {} changed...",
            dependents.len(),
            self.identifier
        );

        for module_id in dependents {
            self.rebuild_module_js(&modules_root, &module_id)?;
        }

        Ok(())
    }

    fn infer_modules_root(&self) -> Option<PathBuf> {
        let mut current = self.input_dir.parent()?.to_path_buf();

        loop {
            if contains_module_manifests(&current) {
                return Some(current);
            }

            let nested = current.join("modules");
            if contains_module_manifests(&nested) {
                return Some(nested);
            }

            let parent = current.parent()?.to_path_buf();
            if parent == current {
                return None;
            }
            current = parent;
        }
    }

    fn discover_transitive_dependents(&self, modules_root: &Path) -> Result<Vec<String>> {
        let mut reverse_graph: HashMap<String, HashSet<String>> = HashMap::new();

        for entry in fs::read_dir(modules_root)
            .with_context(|| format!("Failed to read modules root: {}", modules_root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let module_id = entry.file_name().to_string_lossy().to_string();
            let metadata_path = entry.path().join("metadata.json");
            if !metadata_path.exists() {
                continue;
            }

            let metadata: Metadata = crate::util::read_json(&metadata_path)
                .with_context(|| format!("Failed to parse metadata for module {}", module_id))?;

            for dep in metadata.dependencies.dependency_ids() {
                reverse_graph
                    .entry(dep.to_string())
                    .or_default()
                    .insert(module_id.clone());
            }
        }

        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut ordered = Vec::new();

        queue.push_back(self.identifier.clone());
        visited.insert(self.identifier.clone());

        while let Some(module) = queue.pop_front() {
            if let Some(dependents) = reverse_graph.get(&module) {
                let mut sorted_dependents: Vec<_> = dependents.iter().cloned().collect();
                sorted_dependents.sort();
                for dependent in sorted_dependents {
                    if visited.insert(dependent.clone()) {
                        ordered.push(dependent.clone());
                        queue.push_back(dependent);
                    }
                }
            }
        }

        Ok(ordered)
    }

    fn rebuild_module_js(&self, modules_root: &Path, module_id: &str) -> Result<()> {
        let module_dir = modules_root.join(module_id);
        let metadata_path = module_dir.join("metadata.json");
        let metadata: Metadata = crate::util::read_json(&metadata_path)
            .with_context(|| format!("Failed to read metadata: {}", metadata_path.display()))?;

        if metadata.entries.js.is_none() {
            return Ok(());
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let inputs = collect_js_inputs(&module_dir);
        inputs.par_iter().try_for_each(|input| {
            let rel = input
                .strip_prefix(&module_dir)
                .unwrap_or(input)
                .to_path_buf();
            let mut rel_js = rel.clone();
            rel_js.set_extension("js");
            let output = module_dir.join(&rel_js);

            if !needs_rebuild(input, &output)? {
                return Ok(());
            }

            let rel_js_str = normalize_slashes(&rel_js);
            let filepath = format!("/modules/{}/{}", module_id, rel_js_str);
            self.js_transpiler
                .transpile(input, &output, &filepath, timestamp)
        })?;

        let timestamp_file = module_dir.join("timestamp");
        ensure_parent(&timestamp_file)?;
        fs::write(&timestamp_file, format!("{timestamp}")).with_context(|| {
            format!(
                "Failed to write dependent timestamp: {}",
                timestamp_file.display()
            )
        })?;

        Ok(())
    }
}

pub enum FileType {
    ToJS,
    ToCSS,
    Css,
    Unknown,
}

pub fn parse_file_type(path: &Path, rel: &Path) -> FileType {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "js" => FileType::ToJS,
        "ts" => {
            if rel.to_string_lossy().ends_with(".d.ts") {
                FileType::Unknown
            } else {
                FileType::ToJS
            }
        }
        "mjs" | "jsx" | "tsx" => FileType::ToJS,
        "css" => FileType::Css,
        "scss" => FileType::ToCSS,
        _ => FileType::Unknown,
    }
}

fn collect_js_inputs(root: &Path) -> Vec<PathBuf> {
    let mut inputs = Vec::new();
    for entry in WalkDir::new(root).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        if matches!(parse_file_type(path, &rel), FileType::ToJS) {
            inputs.push(path.to_path_buf());
        }
    }
    inputs.sort();
    inputs
}

fn contains_module_manifests(root: &Path) -> bool {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("metadata.json").exists() {
            return true;
        }
    }

    false
}

fn needs_rebuild(input: &Path, output: &Path) -> Result<bool> {
    let input_meta = match fs::metadata(input) {
        Ok(m) => m,
        Err(_) => return Ok(true),
    };
    let output_meta = match fs::metadata(output) {
        Ok(m) => m,
        Err(_) => return Ok(true),
    };
    let input_mtime = input_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let output_mtime = output_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(input_mtime > output_mtime)
}
