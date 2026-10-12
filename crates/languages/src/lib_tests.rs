use std::path::Path;

use arborium::tree_sitter::{Parser, QueryCursor, StreamingIterator};
use warp_util::standardized_path::StandardizedPath;

use crate::{
    SUPPORTED_LANGUAGES, language_by_filename, language_by_local_filename, language_by_name,
    language_name_for_filename_parts, load_language,
};

/// Resolves a bare filename to its internal language name without loading the grammar.
fn language_name(filename: &str) -> Option<&'static str> {
    let path = Path::new(filename);
    let name = language_name_for_filename_parts(
        path.file_name().and_then(|name| name.to_str()),
        path.extension().and_then(|extension| extension.to_str()),
    );
    if let Some(name) = name {
        assert!(
            SUPPORTED_LANGUAGES.contains(&name),
            "{filename} resolved to {name}, which is not a supported language",
        );
    }
    name
}

/// Parses `source` and returns `(capture name, captured text)` for every highlight capture.
fn highlights(language_name: &str, source: &str) -> Vec<(String, String)> {
    let language = language_by_name(language_name)
        .unwrap_or_else(|| panic!("{language_name} should be a supported language"));
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar)
        .expect("grammar should be compatible with the parser");
    let tree = parser.parse(source, None).expect("source should parse");

    let capture_names = language.highlight_query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(
        &language.highlight_query,
        tree.root_node(),
        source.as_bytes(),
    );
    let mut highlights = Vec::new();
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        highlights.push((
            capture_names[capture.index as usize].to_string(),
            source[capture.node.byte_range()].to_string(),
        ));
    }
    highlights
}

fn assert_highlights(language_name: &str, source: &str, expected: &[(&str, &str)]) {
    let actual = highlights(language_name, source);
    for (name, text) in expected {
        assert!(
            actual
                .iter()
                .any(|(actual_name, actual_text)| actual_name == name && actual_text == text),
            "expected {text:?} to be highlighted as {name} in {language_name}, got {actual:#?}",
        );
    }
}

fn assert_language_names(cases: &[(&str, &str)]) {
    for (filename, expected) in cases {
        assert_eq!(
            language_name(filename),
            Some(*expected),
            "{filename} should resolve to {expected}",
        );
    }
}

/// Validate that every supported language can be loaded successfully.
/// This catches invalid node types, syntax errors, and other issues in .scm query files
/// (highlights, indents, identifiers) that would otherwise only surface at runtime.
#[test]
fn all_supported_languages_load_successfully() {
    let failures: Vec<_> = SUPPORTED_LANGUAGES
        .iter()
        .filter(|lang| load_language(lang).is_none())
        .collect();

    assert!(
        failures.is_empty(),
        "The following languages failed to load:\n{}",
        failures
            .iter()
            .map(|lang| format!("  - {lang}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Both `.html` and the legacy three-character `.htm` extension should resolve to
/// the same HTML language entry. `.htm` is widely produced by static-site generators
/// and historical web tooling (DOS 8.3 filename limits) and is already treated as
/// an HTML/text file elsewhere in the codebase
/// (see `is_development_text_extension` in `crates/warp_util/src/file_type.rs`).
#[test]
fn html_extensions_resolve_to_html() {
    for filename in ["index.html", "index.htm"] {
        let path = StandardizedPath::try_new(&format!("/tmp/{filename}"))
            .expect("test path should be absolute");
        let language = language_by_filename(&path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "HTML",
            "{filename} should resolve to HTML",
        );
    }
}

#[test]
fn local_html_extensions_resolve_to_html() {
    for filename in ["index.html", "index.htm"] {
        let path = Path::new(filename);
        let language = language_by_local_filename(path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "HTML",
            "{filename} should resolve to HTML",
        );
    }
}

/// `.command` is the macOS convention for double-clickable shell scripts.
/// Make sure `language_by_filename` recognizes it as shell so the editor
/// renders syntax highlighting instead of the
/// "Language support is unavailable for this file type" footer.
#[test]
fn command_extension_resolves_to_shell() {
    let path =
        StandardizedPath::try_new("/tmp/script.command").expect("test path should be absolute");
    let language =
        language_by_filename(&path).expect("`.command` files should resolve to a language");
    assert_eq!(language.display_name(), "Shell");
}

#[test]
fn local_command_extension_resolves_to_shell() {
    let language = language_by_local_filename(Path::new("script.command"))
        .expect("`.command` files should resolve to a language");
    assert_eq!(language.display_name(), "Shell");
}

/// `.md` and `.markdown` should resolve to the Markdown language so the editor applies
/// syntax highlighting to Markdown source files.
#[test]
fn markdown_extensions_resolve_to_markdown() {
    for filename in ["README.md", "notes.markdown"] {
        let path = StandardizedPath::try_new(&format!("/tmp/{filename}"))
            .expect("test path should be absolute");
        let language = language_by_filename(&path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(
            language.display_name(),
            "Markdown",
            "{filename} should resolve to Markdown",
        );
    }
}

#[test]
fn script_module_and_declaration_extensions_resolve_to_their_base_language() {
    assert_language_names(&[
        ("vite.config.mjs", "javascript"),
        ("legacy.cjs", "javascript"),
        ("worker.mts", "typescript"),
        ("config.cts", "typescript"),
        ("globals.d.ts", "typescript"),
        ("env.d.mts", "typescript"),
        ("page.test.tsx", "tsx"),
        ("stubs.pyi", "python"),
    ]);
}

#[test]
fn config_and_data_variants_resolve_to_their_base_language() {
    assert_language_names(&[
        ("wrangler.jsonc", "json"),
        ("events.jsonl", "json"),
        ("composer.lock", "json"),
        ("bun.lock", "json"),
        ("uv.lock", "toml"),
        ("Cargo.lock", "toml"),
        ("docker-compose.yml", "yaml"),
        (".prettierrc", "yaml"),
        ("main.tf", "hcl"),
        ("prod.tfvars", "hcl"),
        (".terraform.lock.hcl", "hcl"),
        ("icon.svg", "xml"),
        ("Info.plist", "xml"),
    ]);
}

#[test]
fn dotenv_and_shell_startup_files_resolve_to_shell() {
    assert_language_names(&[
        (".env", "shell"),
        (".env.local", "shell"),
        (".env.example", "shell"),
        (".env.production.local", "shell"),
        ("staging.env", "shell"),
        (".envrc", "shell"),
        (".dev.vars", "shell"),
        (".zshenv", "shell"),
        (".profile", "shell"),
        ("deploy.zsh", "shell"),
        ("install.bash", "shell"),
    ]);
}

#[test]
fn dockerfile_variants_resolve_to_dockerfile() {
    assert_language_names(&[
        ("Dockerfile", "dockerfile"),
        ("Dockerfile.dev", "dockerfile"),
        ("Containerfile.prod", "dockerfile"),
        ("api.Dockerfile", "dockerfile"),
        ("web.dockerfile", "dockerfile"),
    ]);
}

#[test]
fn template_dialects_fall_back_to_their_host_language() {
    assert_language_names(&[
        ("welcome.blade.php", "html"),
        ("Welcome.Blade.PHP", "html"),
        ("index.php", "php"),
        ("invite.hbs", "html"),
        ("post.mdx", "markdown"),
        ("project.mdc", "markdown"),
    ]);
}

#[test]
fn postgres_extensions_resolve_to_sql() {
    assert_language_names(&[
        ("20240101_init.sql", "sql"),
        ("functions.pgsql", "sql"),
        ("seed.psql", "sql"),
        ("triggers.plpgsql", "sql"),
    ]);
}

#[test]
fn ruby_project_files_resolve_to_ruby() {
    assert_language_names(&[
        ("Gemfile", "ruby"),
        ("Podfile", "ruby"),
        ("tasks.rake", "ruby"),
    ]);
}

#[test]
fn extensions_resolve_case_insensitively() {
    assert_language_names(&[
        ("README.MD", "markdown"),
        ("DATA.JSON", "json"),
        ("legacy.H", "cpp"),
    ]);
}

#[test]
fn unmapped_files_resolve_to_no_language() {
    for filename in ["notes.txt", "data.csv", ".gitignore", "environment", "env"] {
        assert_eq!(
            language_name(filename),
            None,
            "{filename} should not resolve to a language",
        );
    }
}

#[test]
fn sql_highlights_numbers_and_plpgsql_function_bodies() {
    let source = "\
create or replace function public.bump(amount integer)
returns integer
language plpgsql
as $$
begin
  update counters set total = total + 42 where id = 1.5;
  return amount;
end;
$$;
";
    assert_highlights(
        "sql",
        source,
        &[
            ("keyword", "create"),
            ("keyword", "function"),
            ("keyword", "begin"),
            ("keyword", "update"),
            ("number", "42"),
            ("number", "1.5"),
        ],
    );
}

#[test]
fn dotenv_highlights_as_shell_assignments() {
    let source = "# Database\nDATABASE_URL=\"postgres://localhost/db\"\nexport PORT=3000\n";
    assert_highlights(
        "shell",
        source,
        &[
            ("comment", "# Database"),
            ("property", "DATABASE_URL"),
            ("string", "\"postgres://localhost/db\""),
            ("keyword", "export"),
            ("property", "PORT"),
        ],
    );
}

#[test]
fn blade_and_handlebars_markup_highlights_as_html() {
    let source = "\
@if ($user)
    <h1 class=\"title\">{{ $user->name }}</h1>
@endif
";
    assert_highlights(
        "html",
        source,
        &[("tag", "h1"), ("attribute", "class"), ("string", "title")],
    );
}

#[test]
fn stylesheet_markup_and_config_extensions_resolve_to_their_grammar() {
    assert_language_names(&[
        ("theme.scss", "scss"),
        ("App.svelte", "svelte"),
        ("report.typ", "typst"),
        ("alembic.ini", "ini"),
        ("setup.cfg", "ini"),
        ("app.service", "ini"),
        (".npmrc", "ini"),
        (".editorconfig", "ini"),
        ("fix.patch", "diff"),
        ("changes.diff", "diff"),
    ]);
}

#[test]
fn scss_highlights_rules_and_sass_directives() {
    let source = "\
// brand
$primary: #673ab7;
@mixin rounded($radius: 4px) {
  border-radius: $radius;
}
.card {
  @include rounded(8px);
}
";
    assert_highlights(
        "scss",
        source,
        &[
            ("comment", "// brand"),
            ("property", "$primary"),
            ("keyword", "@mixin"),
            ("function", "rounded"),
            ("property", "border-radius"),
            ("keyword.import", "@include"),
            ("number", "8px"),
        ],
    );
}

#[test]
fn svelte_highlights_markup_and_logic_blocks() {
    let source = "\
<main class=\"app\">
  {#if count > 0}
    <p>Hello</p>
  {/if}
</main>
";
    assert_highlights(
        "svelte",
        source,
        &[
            ("tag", "main"),
            ("attribute", "class"),
            ("string", "app"),
            ("keyword.conditional", "if"),
        ],
    );
}

#[test]
fn typst_highlights_markup_and_code() {
    let source = "\
#set page(paper: \"a4\", margin: 2cm)
= Introduction
// note
";
    assert_highlights(
        "typst",
        source,
        &[
            ("keyword.control", "set"),
            ("function", "page"),
            ("string", "\"a4\""),
            ("constant.numeric", "2cm"),
            ("markup.heading.1", "= Introduction"),
            ("comment", "// note"),
        ],
    );
}

#[test]
fn ini_highlights_keys_with_and_without_a_section() {
    let source = "\
# npm
registry=https://registry.npmjs.org/
[Unit]
Description=Worker
";
    assert_highlights(
        "ini",
        source,
        &[
            ("comment", "# npm\n"),
            ("property", "registry"),
            ("type", "Unit"),
            ("property", "Description"),
        ],
    );
}

#[test]
fn diff_highlights_headers_and_changed_lines() {
    let source = "\
diff --git a/src/main.rs b/src/main.rs
index 83db48f..bf2a3c1 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,2 @@
-old
+new
";
    assert_highlights(
        "diff",
        source,
        &[
            ("function", "diff --git a/src/main.rs b/src/main.rs"),
            ("number", "83db48f"),
            ("attribute", "@@ -1,2 +1,2 @@"),
            ("keyword", "-old"),
            ("string", "+new"),
        ],
    );
}

/// Filename-only matches must work through both public lookups, which split paths differently.
#[test]
fn filename_matches_resolve_through_both_lookups() {
    for (filename, expected) in [(".env.local", "Shell"), ("welcome.blade.php", "HTML")] {
        let path = StandardizedPath::try_new(&format!("/tmp/{filename}"))
            .expect("test path should be absolute");
        let language = language_by_filename(&path)
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(language.display_name(), expected);

        let language = language_by_local_filename(Path::new(filename))
            .unwrap_or_else(|| panic!("expected {filename} to resolve to a language"));
        assert_eq!(language.display_name(), expected);
    }
}
