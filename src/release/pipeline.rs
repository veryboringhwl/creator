use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde::Serialize;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::build::{Builder, BuilderOutcome, DriverOptions};
use crate::core::classmap::{self, ClassmapInfo};
use crate::core::env::BuildEnvironment;
use crate::core::metadata::Metadata;
use crate::core::{Error, Plan, Result, util};

#[derive(Debug)]
pub struct ReleaseSpec {
    pub inputs: Vec<PathBuf>,
    pub classmap_url: String,
    pub output_dir: PathBuf,
    pub dev: bool,
}

#[derive(Debug, Serialize)]
pub struct ArtifactEntry {
    pub id: String,
    pub version: String,
    pub zip: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct FailedEntry {
    pub dir: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ReleaseManifest {
    pub artifacts: Vec<ArtifactEntry>,
    pub failed: Vec<FailedEntry>,
    pub classmap_url: String,
    pub classmap_version: String,
}

pub fn run(spec: ReleaseSpec) -> Result<ReleaseManifest> {
    let info = classmap::fetch_classmap_info(&spec.classmap_url)?;
    let classmap_semver = classmap::classmap_semver_display(info.version);

    util::ensure_dir(&spec.output_dir)?;

    let inputs = if spec.inputs.is_empty() {
        classmap::discover_module_dirs(Path::new("modules"))?
    } else {
        spec.inputs
    };

    let results: Vec<(PathBuf, Result<ArtifactEntry>)> = inputs
        .par_iter()
        .map(|input| {
            let result = build_and_zip(input, &info, &spec.output_dir, spec.dev);
            (input.clone(), result)
        })
        .collect();

    let mut artifacts = Vec::new();
    let mut failed = Vec::new();
    for (input, result) in results {
        match result {
            Ok(entry) => artifacts.push(entry),
            Err(err) => failed.push(FailedEntry {
                dir: input.display().to_string(),
                error: format!("{err:#}"),
            }),
        }
    }

    Ok(ReleaseManifest {
        artifacts,
        failed,
        classmap_url: spec.classmap_url,
        classmap_version: classmap_semver,
    })
}

fn build_and_zip(
    input_dir: &Path,
    info: &ClassmapInfo,
    output_root: &Path,
    dev: bool,
) -> Result<ArtifactEntry> {
    let metadata_path = input_dir.join("metadata.json");
    let metadata: Metadata = util::read_json(&metadata_path)?;
    let mut metadata_value: serde_json::Value = util::read_json_value(&metadata_path)?;

    let identifier = metadata.name.clone();
    let base_version = metadata.version.clone();
    let classmap_semver = classmap::classmap_semver_display(info.version);
    let full_version = format!("{base_version}+{classmap_semver}");
    metadata_value["version"] = serde_json::Value::String(full_version.clone());

    let scratch_dir = output_root.join(format!(".scratch-{}", util::unix_millis_now()));
    util::ensure_dir(&scratch_dir)?;

    let env = Arc::new(BuildEnvironment::current(dev, false));
    let plan = Plan {
        metadata: metadata.clone(),
        source_dir: input_dir.to_path_buf(),
        output_dir: scratch_dir.clone(),
        classmap: Arc::new(info.mapping.clone()),
        env,
    };
    let builder = Builder::with_options(
        plan,
        DriverOptions {
            include_unknown: true,
        },
    )?;
    let outcome: BuilderOutcome = builder
        .run()
        .map_err(|e| Error::Release(format!("build {identifier}: {e:#}")))?;
    tracing::info!(?outcome, identifier, "built module");

    let out_metadata_path = scratch_dir.join("metadata.json");
    let out_metadata_json = serde_json::to_string(&metadata_value)
        .map_err(|e| Error::Release(format!("serialize metadata: {e}")))?;
    util::write_text(&out_metadata_path, &out_metadata_json)?;

    let zip_name = format!(
        "{}.zip",
        sanitize_zip_name(&format!("{identifier}@{full_version}"))
    );
    let zip_path = output_root.join(&zip_name);
    zip_directory(&scratch_dir, &zip_path)?;

    let _ = fs::remove_dir_all(&scratch_dir);

    Ok(ArtifactEntry {
        id: identifier,
        version: full_version,
        zip: zip_name,
        metadata: metadata_value,
    })
}

fn sanitize_zip_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ' ' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn zip_directory(src: &Path, dest: &Path) -> Result<()> {
    let file = File::create(dest).map_err(|source| Error::io(dest, source))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in WalkDir::new(src).min_depth(1) {
        let entry = entry.map_err(|source| Error::io(src, source.into()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .strip_prefix(src)
            .map_err(|e| Error::Zip(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        zip.start_file(name, options)
            .map_err(|e| Error::Zip(e.to_string()))?;
        let mut f = File::open(path).map_err(|source| Error::io(path, source))?;
        io::copy(&mut f, &mut zip).map_err(|source| Error::io(path, source))?;
    }

    zip.finish().map_err(|e| Error::Zip(e.to_string()))?;
    Ok(())
}
