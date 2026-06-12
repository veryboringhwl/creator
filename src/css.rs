use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use regex::Regex;

use crate::classmap::Mapping;
use crate::util::{ensure_parent, normalize_slashes};

pub struct CssTranspiler {
    mapping: Mapping,
}

impl CssTranspiler {
    pub fn new(mapping: Mapping) -> Self {
        Self { mapping }
    }

    pub fn transpile_scss(&self, input: &Path, output: &Path, files: &[PathBuf]) -> Result<()> {
        let mut css = grass::from_path(
            input,
            &grass::Options::default().style(grass::OutputStyle::Expanded),
        )
        .with_context(|| format!("Failed to compile scss: {}", input.display()))?;

        if css.contains("@tailwind") {
            css = run_tailwind(&css, files)?;
        }

        let css = self.process_css(&css)?;
        write_css(output, &css)?;
        Ok(())
    }

    pub fn transpile_css(&self, input: &Path, output: &Path) -> Result<()> {
        let css = fs::read_to_string(input)
            .with_context(|| format!("Failed to read CSS file: {}", input.display()))?;
        let css = self.process_css(&css)?;
        write_css(output, &css)?;
        Ok(())
    }

    fn process_css(&self, css: &str) -> Result<String> {
        let mut css = autoprefix_css(css)?;
        css = remap_css_selectors(&css, &self.mapping)?;
        css = remap_css_classmap_expressions(&css, &self.mapping)?;
        Ok(css)
    }
}

fn write_css(output: &Path, css: &str) -> Result<()> {
    ensure_parent(output)?;
    fs::write(output, css)
        .with_context(|| format!("Failed to write css file: {}", output.display()))?;
    Ok(())
}

fn autoprefix_css(css: &str) -> Result<String> {
    let stylesheet = lightningcss::stylesheet::StyleSheet::parse(
        css,
        lightningcss::stylesheet::ParserOptions::default(),
    )
    .map_err(|err| anyhow!("Failed to parse CSS: {err}"))?;

    let result = stylesheet
        .to_css(lightningcss::stylesheet::PrinterOptions {
            minify: false,
            targets: lightningcss::targets::Targets::default(),
            ..Default::default()
        })
        .map_err(|err| anyhow!("Failed to serialize CSS: {err}"))?;

    Ok(result.code)
}

fn run_tailwind(css: &str, files: &[PathBuf]) -> Result<String> {
    let bin = std::env::var("TAILWINDCSS_BIN").unwrap_or_else(|_| "tailwindcss".to_string());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let tmp_dir = std::env::temp_dir();
    let input_path = tmp_dir.join(format!("spicetify-tailwind-{stamp}-input.css"));
    let output_path = tmp_dir.join(format!("spicetify-tailwind-{stamp}-output.css"));

    fs::write(&input_path, css)
        .with_context(|| format!("Failed to write temp css: {}", input_path.display()))?;

    let mut cmd = Command::new(&bin);
    cmd.arg("--input")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_path);

    if !files.is_empty() {
        let content = files
            .iter()
            .map(|path| normalize_slashes(path))
            .collect::<Vec<_>>()
            .join(",");
        cmd.arg("--content").arg(content);
    }

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("tailwindcss CLI not found. Set TAILWINDCSS_BIN or install tailwindcss")
        } else {
            anyhow!("Failed to run tailwindcss: {err}")
        }
    })?;

    if !status.success() {
        return Err(anyhow!("tailwindcss failed with status: {status}"));
    }

    let output_css = fs::read_to_string(&output_path)
        .with_context(|| format!("Failed to read temp css: {}", output_path.display()))?;

    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_file(&output_path);

    Ok(output_css)
}

fn remap_css_selectors(css: &str, mapping: &Mapping) -> Result<String> {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    let mut cursor = 0;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut stack: Vec<bool> = Vec::new();

    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();

        if in_comment {
            if c == b'*' && next == Some(b'/') {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        if let Some(quote) = in_string {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'/' && next == Some(b'*') {
            in_comment = true;
            i += 2;
            continue;
        }

        if c == b'\'' || c == b'"' {
            in_string = Some(c);
            i += 1;
            continue;
        }

        if c == b'{' {
            let prelude = &css[cursor..i];
            let trimmed = prelude.trim_start();
            let inside_keyframes = stack.iter().any(|keyframes| *keyframes);
            if trimmed.starts_with('@') {
                out.push_str(prelude);
                out.push('{');
                stack.push(is_keyframes_prelude(trimmed));
            } else if inside_keyframes {
                out.push_str(prelude);
                out.push('{');
                stack.push(false);
            } else {
                out.push_str(&remap_selector_segment(prelude, mapping)?);
                out.push('{');
                stack.push(false);
            }
            i += 1;
            cursor = i;
            continue;
        }

        if c == b'}' {
            out.push_str(&css[cursor..i]);
            out.push('}');
            if !stack.is_empty() {
                stack.pop();
            }
            i += 1;
            cursor = i;
            continue;
        }

        if c == b';' {
            out.push_str(&css[cursor..i]);
            out.push(';');
            i += 1;
            cursor = i;
            continue;
        }

        i += 1;
    }

    if cursor < css.len() {
        out.push_str(&css[cursor..]);
    }

    Ok(out)
}

fn is_keyframes_prelude(prelude: &str) -> bool {
    let prelude = prelude.trim_start();
    let lower = prelude.to_ascii_lowercase();
    lower.starts_with("@keyframes")
        || lower.starts_with("@-webkit-keyframes")
        || lower.starts_with("@-moz-keyframes")
        || lower.starts_with("@-ms-keyframes")
}

fn remap_selector_segment(segment: &str, mapping: &Mapping) -> Result<String> {
    let mut out = String::with_capacity(segment.len());
    let bytes = segment.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    let mut attr_depth = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_string {
            if c == b'\\' {
                out.push(c as char);
                if let Some(next) = bytes.get(i + 1) {
                    out.push(*next as char);
                }
                i += 2;
                continue;
            }
            out.push(c as char);
            if c == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == b'\'' || c == b'"' {
            in_string = Some(c);
            out.push(c as char);
            i += 1;
            continue;
        }

        if c == b'[' {
            attr_depth += 1;
            out.push('[');
            i += 1;
            continue;
        }

        if c == b']' {
            if attr_depth > 0 {
                attr_depth -= 1;
            }
            out.push(']');
            i += 1;
            continue;
        }

        if c == b'.' && attr_depth == 0 {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() {
                let ch = bytes[end] as char;
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    end += 1;
                } else {
                    break;
                }
            }

            if end == start {
                out.push('.');
                i += 1;
                continue;
            }

            let name = &segment[start..end];
            let remapped = remap_class_name(name, mapping)?;
            out.push('.');
            out.push_str(remapped.as_deref().unwrap_or(name));
            i = end;
            continue;
        }

        out.push(c as char);
        i += 1;
    }

    Ok(out)
}

fn remap_class_name(name: &str, mapping: &Mapping) -> Result<Option<String>> {
    let idents: Vec<&str> = name.split("__").collect();
    let mut current = mapping;
    let mut last_ident = 0;

    for ident in &idents {
        match current {
            Mapping::Map(map) => match map.get(*ident) {
                Some(next) => {
                    current = next;
                    last_ident += 1;
                }
                None => break,
            },
            _ => break,
        }
    }

    if last_ident == 0 {
        return Ok(None);
    }

    if last_ident != idents.len() {
        let problematic = idents[..=last_ident].join("__");
        return Err(anyhow!(
            "{} isn't a node of the provided mapping",
            problematic
        ));
    }

    match current {
        Mapping::Str(value) => Ok(Some(value.clone())),
        _ => Err(anyhow!(
            "{} isn't an ending node (leaf) of the provided mapping",
            name
        )),
    }
}

fn remap_css_classmap_expressions(css: &str, mapping: &Mapping) -> Result<String> {
    let re = Regex::new(r"\.MAP(?:__[A-Za-z0-9_-]+)+").unwrap();
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;

    for m in re.find_iter(css) {
        out.push_str(&css[cursor..m.start()]);
        let matched = m.as_str();
        let path_text = &matched[1..];
        let path: Vec<&str> = path_text.split("__").collect();
        if let Some(value) = lookup_mapping_path(mapping, &path, "__")? {
            out.push('.');
            out.push_str(&value);
        } else {
            out.push_str(matched);
        }
        cursor = m.end();
    }

    out.push_str(&css[cursor..]);
    Ok(out)
}

fn lookup_mapping_path(mapping: &Mapping, path: &[&str], sep: &str) -> Result<Option<String>> {
    let mut current = mapping;
    let mut last_ident = 0;

    for ident in path {
        match current {
            Mapping::Map(map) => match map.get(*ident) {
                Some(next) => {
                    current = next;
                    last_ident += 1;
                }
                None => break,
            },
            _ => break,
        }
    }

    if last_ident == 0 {
        return Ok(None);
    }

    if last_ident != path.len() {
        return Err(anyhow!(
            "{} isn't a node of the provided mapping",
            path[..=last_ident].join(sep)
        ));
    }

    match current {
        Mapping::Str(value) => Ok(Some(value.clone())),
        _ => Err(anyhow!(
            "{} isn't an ending node (leaf) of the provided mapping",
            path.join(sep)
        )),
    }
}
