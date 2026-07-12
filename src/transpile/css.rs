use std::fs;
use std::path::Path;
use std::process::Command;

use crate::build::SourceNode;
use crate::core::{CssMappingRef, Error, Mapping, Result, ensure_parent};
use crate::transpile::{Transpile, TranspileContext, TranspileOutput};

pub struct CssTranspiler {
    classmap: Mapping,
}

impl CssTranspiler {
    pub fn new(classmap: Mapping) -> Self {
        Self { classmap }
    }

    fn css_ref(&self) -> CssMappingRef<'_> {
        CssMappingRef::new(&self.classmap)
    }
}

impl Transpile for CssTranspiler {
    fn kind(&self) -> crate::build::SourceKind {
        crate::build::SourceKind::Css
    }

    fn transpile(&self, node: &SourceNode, ctx: &TranspileContext) -> Result<TranspileOutput> {
        let css = match node.kind {
            crate::build::SourceKind::Scss => compile_scss(node)?,
            crate::build::SourceKind::Css => node.content.clone(),
            other => {
                return Err(Error::transpile(
                    node.path.clone(),
                    format!("CssTranspiler called with non-CSS kind {other:?}"),
                ));
            }
        };
        let css = if css.contains("@tailwind") {
            expand_tailwind(&css, ctx)?
        } else {
            css
        };
        let processed = process(&css, &self.css_ref())?;
        Ok(TranspileOutput {
            code: processed,
            source_map: None,
        })
    }
}

fn compile_scss(node: &SourceNode) -> Result<String> {
    let options = grass::Options::default().style(grass::OutputStyle::Expanded);
    grass::from_path(&node.path, &options)
        .map_err(|e| Error::transpile(node.path.clone(), format!("SCSS compile: {e}")))
}

fn expand_tailwind(css: &str, ctx: &TranspileContext) -> Result<String> {
    let bin = std::env::var("TAILWINDCSS_BIN").unwrap_or_else(|_| "tailwindcss".to_string());
    let scratch = ctx.scratch_session();
    let input_path = scratch.next_path("css");
    let output_path = scratch.next_path("css");

    fs::write(&input_path, css).map_err(|source| Error::io(&input_path, source))?;

    let mut cmd = Command::new(&bin);
    cmd.arg("--input")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_path);

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::transpile(
                "<tailwind>",
                "tailwindcss CLI not found. Set TAILWINDCSS_BIN or install tailwindcss".to_string(),
            )
        } else {
            Error::transpile("<tailwind>", format!("failed to run: {err}"))
        }
    })?;

    if !status.success() {
        return Err(Error::transpile("<tailwind>", format!("status: {status}")));
    }

    let output_css =
        fs::read_to_string(&output_path).map_err(|source| Error::io(&output_path, source))?;

    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_file(&output_path);

    Ok(output_css)
}

fn process(css: &str, css_mapping: &CssMappingRef<'_>) -> Result<String> {
    let css = autoprefix(css)?;
    let css = remap_selectors(&css, css_mapping)?;
    let css = remap_map_expressions(&css, css_mapping)?;
    Ok(css)
}

fn autoprefix(css: &str) -> Result<String> {
    let stylesheet = lightningcss::stylesheet::StyleSheet::parse(
        css,
        lightningcss::stylesheet::ParserOptions::default(),
    )
    .map_err(|err| Error::transpile("<css>", format!("lightningcss parse: {err}")))?;

    let result = stylesheet
        .to_css(lightningcss::stylesheet::PrinterOptions {
            minify: false,
            targets: lightningcss::targets::Targets::default(),
            ..Default::default()
        })
        .map_err(|err| Error::transpile("<css>", format!("lightningcss serialize: {err}")))?;

    Ok(result.code)
}

fn remap_selectors(css: &str, mapping: &CssMappingRef<'_>) -> Result<String> {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut cursor = 0;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    let mut keyframes_depth: Vec<bool> = Vec::new();

    let mut i = 0;
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
            let inside_keyframes = keyframes_depth.iter().any(|k| *k);
            if trimmed.starts_with('@') {
                out.push_str(prelude);
                out.push('{');
                keyframes_depth.push(is_keyframes_prelude(trimmed));
            } else if inside_keyframes {
                out.push_str(prelude);
                out.push('{');
                keyframes_depth.push(false);
            } else {
                out.push_str(&remap_selector_prelude(prelude, mapping)?);
                out.push('{');
                keyframes_depth.push(false);
            }
            i += 1;
            cursor = i;
            continue;
        }

        if c == b'}' {
            out.push_str(&css[cursor..i]);
            out.push('}');
            if !keyframes_depth.is_empty() {
                keyframes_depth.pop();
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
    let lower = prelude.trim_start().to_ascii_lowercase();
    lower.starts_with("@keyframes")
        || lower.starts_with("@-webkit-keyframes")
        || lower.starts_with("@-moz-keyframes")
        || lower.starts_with("@-ms-keyframes")
}

fn remap_selector_prelude(prelude: &str, mapping: &CssMappingRef<'_>) -> Result<String> {
    let mut out = String::with_capacity(prelude.len());
    let bytes = prelude.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    let mut bracket_depth: i32 = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_string {
            out.push(c as char);
            if c == b'\\' {
                if let Some(next) = bytes.get(i + 1) {
                    out.push(*next as char);
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
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
            bracket_depth += 1;
            out.push('[');
            i += 1;
            continue;
        }

        if c == b']' {
            if bracket_depth > 0 {
                bracket_depth -= 1;
            }
            out.push(']');
            i += 1;
            continue;
        }

        if c == b'.' && bracket_depth == 0 {
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

            let name = &prelude[start..end];
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

fn remap_class_name(name: &str, mapping: &CssMappingRef<'_>) -> Result<Option<String>> {
    Ok(mapping.lookup(name)?.map(str::to_string))
}

fn remap_map_expressions(css: &str, mapping: &CssMappingRef<'_>) -> Result<String> {
    let re = regex::Regex::new(r"\.MAP(?:__[A-Za-z0-9_-]+)+").unwrap();
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;

    for m in re.find_iter(css) {
        out.push_str(&css[cursor..m.start()]);
        let matched = m.as_str();
        let path_text = &matched[5..];
        match mapping.lookup(path_text)? {
            Some(value) => {
                out.push('.');
                out.push_str(value);
            }
            None => out.push_str(matched),
        }
        cursor = m.end();
    }

    out.push_str(&css[cursor..]);
    Ok(out)
}

pub fn write_output(output: &Path, css: &str) -> Result<()> {
    ensure_parent(output)?;
    fs::write(output, css).map_err(|source| Error::io(output, source))
}
