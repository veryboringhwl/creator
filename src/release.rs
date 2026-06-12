use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::builder::{BuildOpts, Builder, BuilderOpts, Metadata};
use crate::classmap::{ClassmapInfo, fetch_classmap_info, reformat_mapping_for_css};
use crate::css::CssTranspiler;
use crate::js::JsTranspiler;
use crate::util::write_text;

pub struct ReleaseOpts {
    pub inputs: Vec<PathBuf>,
    pub classmap_url: String,
    pub output_dir: PathBuf,
}

#[derive(serde::Serialize)]
struct ArtifactEntry {
    id: String,
    version: String,
    zip: String,
    metadata: serde_json::Value,
}

#[derive(serde::Serialize)]
struct FailedEntry {
    dir: String,
    error: String,
}

#[derive(serde::Serialize)]
struct ReleaseManifest {
    artifacts: Vec<ArtifactEntry>,
    failed: Vec<FailedEntry>,
    classmap_url: String,
    classmap_version: String,
}

pub fn run_release(opts: ReleaseOpts) -> Result<()> {
    let info = fetch_classmap_info(&opts.classmap_url)?;
    let classmap_semver = classmap_semver_display(info.version);

    fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("Failed to create output dir: {}", opts.output_dir.display()))?;

    let input_dirs = if opts.inputs.is_empty() {
        crate::classmap::discover_module_dirs(Path::new("modules"))?
    } else {
        opts.inputs
    };

    let results: Vec<Result<ArtifactEntry>> = input_dirs
        .par_iter()
        .map(|input_dir| build_and_zip(input_dir, &info, &opts.output_dir, &classmap_semver))
        .collect();

    let mut artifacts = Vec::new();
    let mut failed = Vec::new();

    for result in results {
        match result {
            Ok(entry) => {
                println!("  {}", entry.zip);
                artifacts.push(entry);
            }
            Err(err) => {
                eprintln!("  FAILED: {:#}", err);
                let label = err
                    .chain()
                    .find_map(|c| c.downcast_ref::<BuildError>())
                    .map(|be| be.module_dir.clone())
                    .unwrap_or_else(|| "unknown module".to_string());
                failed.push(FailedEntry {
                    dir: label,
                    error: format!("{:#}", err),
                });
            }
        }
    }

    let manifest = ReleaseManifest {
        artifacts,
        failed,
        classmap_url: opts.classmap_url,
        classmap_version: classmap_semver,
    };

    let manifest_path = opts.output_dir.join("release-manifest.json");
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("Failed to serialize manifest")?;
    write_text(&manifest_path, &manifest_json)?;

    println!(
        "\n{} artifacts, {} failed — manifest written to {}",
        manifest.artifacts.len(),
        manifest.failed.len(),
        manifest_path.display()
    );

    Ok(())
}

#[derive(Debug)]
struct BuildError {
    module_dir: String,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "build error in {}", self.module_dir)
    }
}

impl std::error::Error for BuildError {}

fn build_and_zip(
    input_dir: &Path,
    info: &ClassmapInfo,
    output_root: &Path,
    classmap_semver: &str,
) -> Result<ArtifactEntry> {
    let metadata_path = input_dir.join("metadata.json");
    let metadata_raw = fs::read_to_string(&metadata_path).map_err(|_| {
        let e: Box<dyn std::error::Error + Send + Sync> = Box::new(BuildError {
            module_dir: input_dir.display().to_string(),
        });
        anyhow::Error::msg("missing metadata.json").context(e)
    })?;
    let metadata: Metadata = serde_json::from_str(&metadata_raw)
        .with_context(|| format!("Failed to parse {}", metadata_path.display()))?;
    let mut metadata_value: serde_json::Value = serde_json::from_str(&metadata_raw)
        .with_context(|| format!("Failed to parse JSON in {}", metadata_path.display()))?;

    let identifier = metadata
        .name
        .clone()
        .ok_or_else(|| anyhow::anyhow!("metadata.name is missing in {}", input_dir.display()))?;

    let base_version = metadata_value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0");
    let full_version = format!("{}+{}", base_version, classmap_semver);
    metadata_value["version"] = serde_json::Value::String(full_version.clone());

    let fingerprint = format!("{}@v{}", identifier, full_version);
    let build_dir = output_root.join(&fingerprint);

    fs::create_dir_all(&build_dir)
        .with_context(|| format!("Failed to create build dir: {}", build_dir.display()))?;

    let js_transpiler = JsTranspiler::new(info.mapping.clone(), false, false);
    let css_mapping = reformat_mapping_for_css(&info.mapping);
    let css_transpiler = CssTranspiler::new(css_mapping);

    let builder = Builder::new(
        js_transpiler,
        css_transpiler,
        false,
        BuilderOpts {
            metadata,
            identifier: identifier.clone(),
            input_dir: input_dir.to_path_buf(),
            output_dir: build_dir.clone(),
        },
    )?;

    builder
        .build(BuildOpts {
            js: true,
            css: true,
            unknown: true,
        })
        .with_context(|| format!("Build failed for {}", fingerprint))?;

    let out_metadata_path = build_dir.join("metadata.json");
    let out_metadata_json =
        serde_json::to_string(&metadata_value).context("Failed to serialize metadata")?;
    write_text(&out_metadata_path, &out_metadata_json)?;

    let zip_name = format!("{}.zip", fingerprint);
    let zip_path = output_root.join(&zip_name);
    zip_directory(&build_dir, &zip_path)?;

    let _ = fs::remove_dir_all(&build_dir);

    Ok(ArtifactEntry {
        id: identifier,
        version: full_version,
        zip: zip_name,
        metadata: metadata_value,
    })
}

fn zip_directory(src: &Path, dest: &Path) -> Result<()> {
    let file = File::create(dest)
        .with_context(|| format!("Failed to create zip file: {}", dest.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in WalkDir::new(src).min_depth(1) {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .strip_prefix(src)
            .with_context(|| format!("Failed to strip prefix from {}", path.display()))?
            .to_string_lossy()
            .into_owned();

        zip.start_file(name, options)
            .with_context(|| format!("Failed to write zip entry for {}", path.display()))?;
        let mut f = File::open(path)
            .with_context(|| format!("Failed to open file for zipping: {}", path.display()))?;
        io::copy(&mut f, &mut zip)
            .with_context(|| format!("Failed to compress {}", path.display()))?;
    }

    zip.finish()
        .with_context(|| format!("Failed to finalize zip: {}", dest.display()))?;
    Ok(())
}

fn classmap_semver_display(version: u64) -> String {
    let major = version / 1_000_000;
    let minor = (version / 1_000) % 1_000;
    let patch = version % 1_000;
    format!("{}.{}.{}", major, minor, patch)
}
