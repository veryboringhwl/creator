use base64::Engine;
use oxc::allocator::{Allocator, FromIn};
use oxc::ast::AstBuilder;
use oxc::ast::ast::{
    ExportAllDeclaration, ExportNamedDeclaration, Expression, ImportDeclaration, MemberExpression, Str
};
use oxc::ast_visit::{VisitMut, walk_mut};
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType};
use oxc::transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer, TypeScriptOptions};

use crate::build::SourceNode;
use crate::core::{Error, Mapping, Result};
use crate::transpile::{Transpile, TranspileContext, TranspileOutput};

pub struct JsTranspiler {
    classmap: Mapping,
    ext_rules: Vec<(regex::Regex, &'static str)>,
}

impl JsTranspiler {
    pub fn new(classmap: Mapping) -> Self {
        let ext_rules: Vec<(regex::Regex, &'static str)> = [
            (r"\.tsx?(\?.*)?$", ".js$1"),
            (r"\.mjs(\?.*)?$", ".js$1"),
            (r"\.mts(\?.*)?$", ".js$1"),
            (r"\.jsx(\?.*)?$", ".js$1"),
        ]
        .into_iter()
        .map(|(pat, rep)| (regex::Regex::new(pat).expect("valid extension regex"), rep))
        .collect();

        Self {
            classmap,
            ext_rules,
        }
    }

    fn rewrite_extension(&self, specifier: &str) -> Option<String> {
        if is_remote(specifier) {
            return None;
        }
        let mut rewritten = specifier.to_string();
        for (regex, replacement) in &self.ext_rules {
            rewritten = regex.replace_all(&rewritten, *replacement).into_owned();
        }
        if rewritten == specifier {
            None
        } else {
            Some(rewritten)
        }
    }

    fn rewrite_specifier(&self, specifier: &str, ctx: &TranspileContext) -> Option<String> {
        if is_remote(specifier) {
            return None;
        }

        if specifier == "/modules/stdlib/src/expose/jsx-runtime" || specifier == "react/jsx-runtime"
        {
            return Some("/modules/stdlib/src/expose/jsx-runtime.js".to_string());
        }

        if ctx.env.dev
            && (specifier.starts_with("./")
                || specifier.starts_with("../")
                || specifier.starts_with('/'))
        {
            let ts = self.resolve_timestamp(specifier, ctx);
            return Some(format!("{specifier}?t={ts}"));
        }

        if let Some(rewritten) = self.rewrite_extension(specifier) {
            return Some(rewritten);
        }

        None
    }

    fn resolve_timestamp(&self, specifier: &str, ctx: &TranspileContext) -> u128 {
        if specifier.starts_with("./") || specifier.starts_with("../") {
            return ctx.timestamp;
        }
        if let Some(module_name) = extract_module_name(specifier)
            && let Some(ts) = ctx.resolve_dep_timestamp(module_name)
        {
            return ts;
        }
        ctx.timestamp
    }

    fn parse<'a>(
        &self,
        allocator: &'a Allocator,
        source: &'a str,
        source_type: SourceType,
    ) -> Result<oxc::ast::ast::Program<'a>> {
        let parser_ret = Parser::new(allocator, source, source_type).parse();
        if !parser_ret.diagnostics.is_empty() {
            return Err(Error::parse(
                "<js>",
                format!("oxc parser errors: {:?}", parser_ret.diagnostics),
            ));
        }
        Ok(parser_ret.program)
    }

    fn transpile_tsx(&self, node: &SourceNode, ctx: &TranspileContext) -> Result<TranspileOutput> {
        let allocator = Allocator::default();
        let source_type = SourceType::tsx();
        let mut program = self.parse(&allocator, &node.content, source_type)?;

        let semantic_ret = SemanticBuilder::new()
            .with_check_syntax_error(true)
            .build(&program);
        if !semantic_ret.diagnostics.is_empty() {
            return Err(Error::parse(
                node.path.clone(),
                format!("oxc semantic errors: {:?}", semantic_ret.diagnostics),
            ));
        }
        let scoping = semantic_ret.semantic.into_scoping();

        let mut ext_rewriter =
            SpecifierRewriter::new(&allocator, |spec| self.rewrite_extension(spec));
        ext_rewriter.visit_program(&mut program);

        let transform_options = TransformOptions {
            typescript: TypeScriptOptions {
                rewrite_import_extensions: None,
                ..TypeScriptOptions::default()
            },
            jsx: JsxOptions {
                runtime: JsxRuntime::Automatic,
                import_source: Some("/modules/stdlib/src/expose".into()),
                ..JsxOptions::default()
            },
            ..TransformOptions::default()
        };

        let ret = Transformer::new(&allocator, &node.path, &transform_options)
            .build_with_scoping(scoping, &mut program);
        if !ret.diagnostics.is_empty() {
            return Err(Error::transpile(
                node.path.clone(),
                format!("oxc transform errors: {:?}", ret.diagnostics),
            ));
        }

        let mut ts_rewriter =
            SpecifierRewriter::new(&allocator, |spec| self.rewrite_specifier(spec, ctx));
        ts_rewriter.visit_program(&mut program);

        let mut map_rewriter = MapExpressionReplacer::new(&allocator, &self.classmap);
        map_rewriter.visit_program(&mut program);

        let codegen_opts = if ctx.env.source_map {
            CodegenOptions {
                source_map_path: Some(node.path.clone()),
                ..CodegenOptions::default()
            }
        } else {
            CodegenOptions::default()
        };
        let codegen_ret = Codegen::new().with_options(codegen_opts).build(&program);
        let mut code = codegen_ret.code;
        let mut source_map = None;

        if let Some(map) = codegen_ret.map {
            let j = map.to_json();
            let value = serde_json::json!({
                "version": j.version,
                "file": j.file,
                "sourceRoot": j.source_root,
                "sources": j.sources,
                "sourcesContent": j.sources_content,
                "names": j.names,
                "mappings": j.mappings,
            });
            let json = serde_json::to_string(&value).map_err(|e| {
                Error::transpile(node.path.clone(), format!("source map serialize: {e}"))
            })?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
            code.push_str("\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,");
            code.push_str(&encoded);
            code.push('\n');
            source_map = Some(json);
        }

        Ok(TranspileOutput { code, source_map })
    }

    fn transpile_plain_js(
        &self,
        node: &SourceNode,
        ctx: &TranspileContext,
    ) -> Result<TranspileOutput> {
        let allocator = Allocator::default();
        let source_type = SourceType::mjs();
        let mut program = self.parse(&allocator, &node.content, source_type)?;

        let mut rewriter =
            SpecifierRewriter::new(&allocator, |spec| self.rewrite_specifier(spec, ctx));
        rewriter.visit_program(&mut program);

        let mut map_rewriter = MapExpressionReplacer::new(&allocator, &self.classmap);
        map_rewriter.visit_program(&mut program);

        let codegen_ret = Codegen::new()
            .with_options(CodegenOptions::default())
            .build(&program);
        Ok(TranspileOutput {
            code: codegen_ret.code,
            source_map: None,
        })
    }
}

impl Transpile for JsTranspiler {
    fn kind(&self) -> crate::build::SourceKind {
        crate::build::SourceKind::Js
    }

    fn transpile(&self, node: &SourceNode, ctx: &TranspileContext) -> Result<TranspileOutput> {
        match node.kind {
            crate::build::SourceKind::Js
            | crate::build::SourceKind::Mjs
            | crate::build::SourceKind::Mts => self.transpile_plain_js(node, ctx),
            crate::build::SourceKind::Ts
            | crate::build::SourceKind::Tsx
            | crate::build::SourceKind::Jsx => self.transpile_tsx(node, ctx),
            other => Err(Error::transpile(
                node.path.clone(),
                format!("JsTranspiler called with non-JS kind {other:?}"),
            )),
        }
    }
}

fn is_remote(specifier: &str) -> bool {
    specifier.starts_with("http://") || specifier.starts_with("https://")
}

fn extract_module_name(specifier: &str) -> Option<&str> {
    let rest = specifier.strip_prefix("/modules/")?;
    rest.split('/').next()
}

struct SpecifierRewriter<'a, F: Fn(&str) -> Option<String>> {
    allocator: &'a Allocator,
    rewrite: F,
}

impl<'a, F: Fn(&str) -> Option<String>> SpecifierRewriter<'a, F> {
    fn new(allocator: &'a Allocator, rewrite: F) -> Self {
        Self { allocator, rewrite }
    }
}

impl<'a, F: Fn(&str) -> Option<String>> VisitMut<'a> for SpecifierRewriter<'a, F> {
    fn visit_import_declaration(&mut self, it: &mut ImportDeclaration<'a>) {
        if let Some(remapped) = (self.rewrite)(it.source.value.as_str()) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_import_declaration(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &mut ExportNamedDeclaration<'a>) {
        if let Some(source) = it.source.as_mut()
            && let Some(remapped) = (self.rewrite)(source.value.as_str())
        {
            source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_export_named_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &mut ExportAllDeclaration<'a>) {
        if let Some(remapped) = (self.rewrite)(it.source.value.as_str()) {
            it.source.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_export_all_declaration(self, it);
    }

    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        if let Expression::ImportExpression(import_expr) = it
            && let Expression::StringLiteral(s) = &mut import_expr.source
            && let Some(remapped) = (self.rewrite)(s.value.as_str())
        {
            s.value = self.allocator.alloc_str(&remapped).into();
        }
        walk_mut::walk_expression(self, it);
    }
}

struct MapExpressionReplacer<'a, 'b> {
    allocator: &'a Allocator,
    mapping: &'b Mapping,
}

impl<'a, 'b> MapExpressionReplacer<'a, 'b> {
    fn new(allocator: &'a Allocator, mapping: &'b Mapping) -> Self {
        Self { allocator, mapping }
    }
}

impl<'a, 'b> VisitMut<'a> for MapExpressionReplacer<'a, 'b> {
    fn visit_expression(&mut self, expr: &mut Expression<'a>) {
        if let Some(me) = expr.as_member_expression()
            && let Some(leaf) = resolve_map_chain(me, self.mapping)
        {
            let span = me.span();
            let ast = AstBuilder::new(self.allocator);
            *expr = Expression::new_string_literal(
                span,
                Str::from_in(leaf.as_str(), self.allocator),
                None,
                &ast,
            );
            return;
        }
        walk_mut::walk_expression(self, expr);
    }
}

fn resolve_map_chain(expr: &MemberExpression<'_>, mapping: &Mapping) -> Option<String> {
    let mut segments: Vec<String> = Vec::new();

    let outermost = expr.static_property_name()?;
    segments.push(outermost.to_string());

    let mut current: &Expression<'_> = expr.object();
    while let Some(inner) = current.as_member_expression() {
        let prop = inner.static_property_name()?;
        segments.push(prop.to_string());
        current = inner.object();
    }

    if !matches!(current, Expression::Identifier(boxed) if boxed.name.as_str() == "MAP") {
        return None;
    }

    segments.reverse();
    let path: Vec<&str> = segments.iter().map(String::as_str).collect();
    mapping.resolve(&path).ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Mapping {
        let json = r#"{ "main": { "topbar": { "wrapper": "X1" } } }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn resolve_map_chain_simple() {
        let m = sample();
        let allocator = Allocator::default();
        let source = "const x = MAP.main.topbar.wrapper;";
        let mut program = Parser::new(&allocator, source, SourceType::mjs())
            .parse()
            .program;
        let mut rewriter = MapExpressionReplacer::new(&allocator, &m);
        rewriter.visit_program(&mut program);
        let codegen = Codegen::new().build(&program);
        assert!(codegen.code.contains("\"X1\""), "got: {}", codegen.code);
        assert!(!codegen.code.contains("MAP"), "got: {}", codegen.code);
    }

    #[test]
    fn resolve_map_chain_unknown_preserved() {
        let m = sample();
        let allocator = Allocator::default();
        let source = "const x = MAP.main.nope;";
        let mut program = Parser::new(&allocator, source, SourceType::mjs())
            .parse()
            .program;
        let mut rewriter = MapExpressionReplacer::new(&allocator, &m);
        rewriter.visit_program(&mut program);
        let codegen = Codegen::new().build(&program);
        assert!(codegen.code.contains("MAP"), "got: {}", codegen.code);
    }

    #[test]
    fn resolve_map_chain_inside_string_literal_preserved() {
        let m = sample();
        let allocator = Allocator::default();
        let source = "const x = \"MAP.main.topbar.wrapper\";";
        let mut program = Parser::new(&allocator, source, SourceType::mjs())
            .parse()
            .program;
        let mut rewriter = MapExpressionReplacer::new(&allocator, &m);
        rewriter.visit_program(&mut program);
        let codegen = Codegen::new().build(&program);
        assert!(codegen.code.contains("\"MAP.main.topbar.wrapper\""));
    }
}
