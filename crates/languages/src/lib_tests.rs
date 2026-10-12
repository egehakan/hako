use std::path::Path;

use arborium::tree_sitter::{Parser, QueryCursor, StreamingIterator};
use rust_embed::RustEmbed;
use warp_util::standardized_path::StandardizedPath;

use crate::{
    Grammars, INHERITS_DIRECTIVE, SUPPORTED_LANGUAGES, is_highlight_capture, language_by_filename,
    language_by_local_filename, language_by_name, language_name_for_filename_parts, load_language,
    load_text, local_highlight_query, query_capture_names,
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
        (".prettierrc", "json"),
        (".clang-format", "yaml"),
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
fn checked_in_templates_resolve_like_the_file_they_stand_in_for() {
    assert_language_names(&[
        ("app.yaml.example", "yaml"),
        ("auth.json.example", "json"),
        ("terraform.tfvars.example", "hcl"),
        ("phpunit.xml.dist", "xml"),
        (".dev.vars.example", "shell"),
        ("env.example", "shell"),
        ("Dockerfile.sample", "dockerfile"),
    ]);
    assert_eq!(language_name("notes.example"), None);
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
fn jsonc_comments_highlight_in_json() {
    let source = "// config\n{\n  /* name */\n  \"name\": \"worker\",\n  \"port\": 8787\n}\n";
    assert_highlights(
        "json",
        source,
        &[
            ("comment", "// config"),
            ("comment", "/* name */"),
            ("string", "\"worker\""),
            ("number", "8787"),
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

#[test]
fn makefile_latex_and_prisma_files_resolve_to_their_grammar() {
    assert_language_names(&[
        ("Makefile", "make"),
        ("makefile", "make"),
        ("GNUmakefile", "make"),
        ("rules.mk", "make"),
        ("thesis.tex", "latex"),
        ("resume.cls", "latex"),
        ("schema.prisma", "prisma"),
    ]);
}

#[test]
fn makefile_highlights_variables_targets_and_directives() {
    let source = "\
# Build
CC := gcc
.PHONY: all
all: $(OBJ)
\t$(CC) -o $@ $^
ifeq ($(DEBUG),1)
include config.mk
endif
";
    assert_highlights(
        "make",
        source,
        &[
            ("comment", "# Build"),
            ("property", "CC"),
            ("string", "gcc"),
            ("keyword", ".PHONY"),
            ("function", "all"),
            ("property", "OBJ"),
            ("property", "$@"),
            ("keyword", "ifeq"),
            ("keyword", "include"),
        ],
    );
}

#[test]
fn latex_highlights_commands_sections_and_references() {
    let source = "\
% preamble
\\usepackage{amsmath}
\\begin{document}
\\section{Introduction}
Some \\textbf{bold} text, $x^2$ \\cite{knuth}.
\\end{document}
";
    assert_highlights(
        "latex",
        source,
        &[
            ("comment", "% preamble"),
            ("keyword", "\\usepackage"),
            ("string", "amsmath"),
            ("keyword", "\\begin"),
            ("type", "document"),
            ("text.title", "Introduction"),
            ("function", "\\textbf"),
            ("string", "$x^2$"),
            ("text.reference", "knuth"),
        ],
    );
}

#[test]
fn prisma_highlights_models_fields_and_attributes() {
    let source = "\
// schema
model User {
  id    Int    @id @default(autoincrement())
  email String @unique
  @@map(\"users\")
}
";
    assert_highlights(
        "prisma",
        source,
        &[
            ("comment", "// schema"),
            ("keyword", "model"),
            ("type", "User"),
            ("type", "Int"),
            ("property", "id"),
            ("property", "default"),
            ("function", "autoincrement"),
            ("property", "@@"),
            ("string", "\"users\""),
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

/// Returns, for each byte of `source`, the name of the capture that would style it: the last
/// capture covering the byte in the order the highlight query yields them, which is how the
/// editor applies captures. Captures that only serve a query's predicates are skipped.
fn painted_captures(language_name: &str, source: &str) -> Vec<Option<String>> {
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
    let mut painted = vec![None; source.len()];
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        let name = capture_names[capture.index as usize];
        if name.starts_with('_') {
            continue;
        }
        painted[capture.node.byte_range()].fill(Some(name.to_string()));
    }
    painted
}

/// Asserts which capture styles the first occurrence of each `text` in `source`, as
/// `(capture name, text)` pairs.
fn assert_painted(language_name: &str, source: &str, expected: &[(&str, &str)]) {
    let painted = painted_captures(language_name, source);
    for (name, text) in expected {
        let start = source
            .find(text)
            .unwrap_or_else(|| panic!("{text:?} should occur in the {language_name} source"));
        let actual = &painted[start..start + text.len()];
        assert!(
            actual
                .iter()
                .all(|capture| capture.as_deref() == Some(*name)),
            "expected {text:?} to be styled as {name} in {language_name}, got {actual:?}",
        );
    }
}

#[test]
fn languages_know_the_name_they_are_registered_under() {
    for name in ["typescript", "shell", "objective-c"] {
        let language = language_by_name(name).expect("language should be supported");
        assert_eq!(language.name, name);
    }
    // An alias resolves to the language's registered name.
    assert_eq!(language_by_name("bash").expect("alias").name, "shell");
}

#[test]
fn query_capture_names_skip_strings_comments_and_private_captures() {
    let query = r#"
; a comment naming @ignored
(identifier) @variable
("@media" @keyword.directive)
((call (identifier) @_callee @function.call)
  (#match? @_callee "^@not_a_capture \" @still_not$"))
"#;
    assert_eq!(
        query_capture_names(query),
        ["variable", "keyword.directive", "function.call"],
    );
}

#[test]
fn highlight_captures_include_enclosing_names() {
    for name in [
        "keyword",
        "function",
        "function.method",
        "string.special",
        "string.special.key",
    ] {
        assert!(is_highlight_capture(name), "{name} should be known");
    }
    for name in [
        "",
        "nonsense",
        "function.nonsense",
        "method.function",
        "_object",
    ] {
        assert!(!is_highlight_capture(name), "{name} should be unknown");
    }
}

/// Every `highlights.scm` must reach a supported language, either as that language's own file
/// or through an `; inherits:` line, so that loading the languages compiles all of them.
#[test]
fn every_local_highlight_query_is_loaded_by_a_supported_language() {
    fn collect_loaded(dir: &str, loaded: &mut Vec<String>) {
        let Some(own) = load_text(&[dir, "highlights.scm"].join("\\")) else {
            return;
        };
        loaded.push(dir.to_string());
        let inherited = own
            .lines()
            .next()
            .and_then(|line| line.strip_prefix(INHERITS_DIRECTIVE));
        for parent in inherited.into_iter().flat_map(|parents| parents.split(',')) {
            collect_loaded(parent.trim(), loaded);
        }
    }

    let mut loaded = Vec::new();
    for lang in SUPPORTED_LANGUAGES {
        collect_loaded(lang, &mut loaded);
        assert!(
            load_language(lang).is_some(),
            "{lang} should load with its highlight query",
        );
    }

    let unused: Vec<_> = <Grammars as RustEmbed>::iter()
        .filter_map(|path| {
            path.strip_suffix("/highlights.scm")
                .map(|dir| dir.to_string())
        })
        .filter(|dir| !loaded.contains(dir))
        .collect();
    assert!(
        unused.is_empty(),
        "no supported language loads the highlights.scm in: {unused:?}",
    );
}

#[test]
fn inherited_highlight_queries_precede_the_inheriting_file() {
    let shared = local_highlight_query("_ecma").expect("shared query should exist");
    let javascript = local_highlight_query("javascript").expect("javascript query should exist");
    let jsx = local_highlight_query("jsx").expect("jsx query should exist");

    let own = load_text("javascript\\highlights.scm").expect("javascript query should exist");

    assert!(javascript.starts_with(&*shared));
    assert!(javascript.contains("(jsx_text) @none"));
    assert!(javascript.ends_with(&*own));
    // A file that only inherits contributes its parent's query.
    assert!(jsx.starts_with(&*javascript));
}

#[test]
fn typescript_distinguishes_declarations_calls_types_and_members() {
    let source = "\
@sealed
class Widget<T> implements Shape {
  private size: number = 1;
  render(target: Canvas): void {
    const label = `size ${this.size}`;
    draw(label);
    target.fill(label.length);
    queue.forEach((item) => parseInt(item));
    new Brush();
  }
}
function build() {}
interface Outline { area?: number }
enum Mode { Fast }
let total = count > 0 ? count : 0;
total += 1;
const pattern = /ab+/gi;
";
    for language in ["typescript", "tsx"] {
        assert_painted(
            language,
            source,
            &[
                ("function.decorator", "sealed"),
                ("type.class", "Widget"),
                ("type.parameter", "T"),
                ("type", "Shape"),
                ("property", "size"),
                ("type.builtin", "number"),
                ("function.method", "render"),
                ("variable.parameter", "target"),
                ("type", "Canvas"),
                ("type.builtin", "void"),
                ("variable.constant", "label"),
                ("string", "`size "),
                ("punctuation.special", "${"),
                ("variable.builtin", "this"),
                ("function.call", "draw"),
                ("function.method.call", "fill"),
                ("property", "length"),
                ("variable.object", "queue"),
                ("function.method.builtin", "forEach"),
                ("keyword.function", "=>"),
                ("function.builtin", "parseInt"),
                ("constructor", "Brush"),
                ("function", "build"),
                ("type.interface", "Outline"),
                ("keyword.operator", "?"),
                ("type.enum", "Mode"),
                ("constant.enum", "Fast"),
                ("variable", "total"),
                ("operator", "="),
                ("punctuation.bracket", "<"),
                ("operator.assignment.compound", "+="),
                ("string.special", "ab+"),
                ("keyword", "gi"),
            ],
        );
    }
}

#[test]
fn tsx_and_javascript_distinguish_jsx_tags_components_and_attributes() {
    let source = "\
import React from \"react\";
export const App = () => (
  <main className=\"app\">
    <Panel.Header title={name} />
    plain
  </main>
);
";
    for language in ["tsx", "javascript", "jsx"] {
        assert_painted(
            language,
            source,
            &[
                ("keyword", "import"),
                ("variable", "React"),
                ("function", "App"),
                ("tag", "main"),
                ("attribute", "className"),
                ("string", "\"app\""),
                ("constructor", "Panel.Header"),
                ("attribute", "title"),
                ("punctuation.special", "{"),
                ("variable", "name"),
                ("none", "plain"),
                ("punctuation.bracket", "/>"),
            ],
        );
    }
}

#[test]
fn jsx_component_keeps_its_color_when_a_function_of_the_same_name_is_declared() {
    let source = "\
export function Card() {
  return (
    <section className=\"card\">
      <Header title={title} />
    </section>
  );
}
function Header(props: { title: string }) {
  return <h2>{props.title}</h2>;
}
";
    assert_painted("tsx", source, &[("constructor", "Header"), ("tag", "h2")]);
}

#[test]
fn javascript_distinguishes_classes_parameters_and_bindings() {
    let source = "\
class Animal extends Base {
  speak(times) { return this.sound; }
}
const view = <li id=\"a\">text<Item /></li>;
const { first } = options;
";
    for language in ["javascript", "jsx"] {
        assert_painted(
            language,
            source,
            &[
                ("type.class", "Animal"),
                ("type", "Base"),
                ("function.method", "speak"),
                ("variable.parameter", "times"),
                ("variable.builtin", "this"),
                ("property", "sound"),
                ("variable.constant", "view"),
                ("tag", "li"),
                ("attribute", "id"),
                ("none", "text"),
                ("constructor", "Item"),
                ("variable.constant", "first"),
                ("variable", "options"),
            ],
        );
    }
}

#[test]
fn json_distinguishes_keys_from_strings() {
    let source = "{ \"name\": \"x\\n\", \"flags\": [true, null], \"on\": false }\n";
    assert_painted(
        "json",
        source,
        &[
            ("string.special.key", "\"name\""),
            ("string", "\"x"),
            ("escape", "\\n"),
            ("constant.builtin.array", "true"),
            ("constant.builtin.array", "null"),
            ("constant.builtin", "false"),
        ],
    );
}

#[test]
fn markdown_distinguishes_heading_list_and_quote_markers() {
    let source = "# Title\n\n- item\n1. first\n\n> quoted\n\n```sh\nls\n```\n\n---\n";
    assert_painted(
        "markdown",
        source,
        &[
            ("punctuation.special.heading", "#"),
            ("text.title", "Title"),
            ("punctuation.special.list", "- "),
            ("punctuation.special.list", "1. "),
            ("markup.quote", "> quoted"),
            ("punctuation.delimiter", "```"),
            ("none", "sh"),
            ("none", "ls"),
            ("punctuation.delimiter", "---"),
        ],
    );
}

#[test]
fn python_distinguishes_parameters_calls_and_builtins() {
    let source = "\
@cache
class Shape(Base):
    def area(self, scale: int, *rest) -> float:
        if scale not in rest and scale:
            raise ValueError(f\"bad {scale!r}\")
        return self.compute(scale, exact=True)
";
    assert_painted(
        "python",
        source,
        &[
            ("function.decorator", "@cache"),
            ("type.class", "Shape"),
            ("type.class", "Base"),
            ("function", "area"),
            ("variable.builtin", "self"),
            ("variable.parameter", "scale"),
            ("type.builtin", "int"),
            ("variable.parameter", "rest"),
            ("punctuation.delimiter", "->"),
            ("type.builtin", "float"),
            ("keyword.operator", "not"),
            ("keyword.operator", "and"),
            ("type.exception", "ValueError"),
            ("punctuation.special", "{"),
            ("keyword.directive", "!r"),
            ("function.method.call", "compute"),
            ("variable.argument", "exact"),
            ("constant.builtin", "True"),
        ],
    );
}

#[test]
fn sql_distinguishes_qualified_names_and_arithmetic() {
    let source = "select u.id, total * 2 from public.users u where u.age > 18;\n";
    assert_painted(
        "sql",
        source,
        &[
            ("keyword", "select"),
            ("variable.qualified", "u"),
            ("variable.qualified", "id"),
            ("field", "total"),
            ("operator.arithmetic", "*"),
            ("number", "2"),
            ("variable.qualified", "public"),
            ("variable.qualified", "users"),
            ("operator", ">"),
        ],
    );
}

#[test]
fn php_distinguishes_variables_types_and_builtin_functions() {
    let source = "\
<?php
namespace App\\Http;
class Greeter extends Base {
    public function __construct(private string $name) {}
    public function greet(): string { echo strlen($this->name); return helper($x) . \"!\"; }
}
";
    assert_painted(
        "php",
        source,
        &[
            ("tag", "<?php"),
            ("module", "App\\Http"),
            ("type.class", "Greeter"),
            ("type", "Base"),
            ("function.builtin", "__construct"),
            ("type.builtin", "string"),
            ("variable", "$name"),
            ("function.method", "greet"),
            ("function.builtin", "echo"),
            ("function.builtin", "strlen"),
            ("variable.builtin", "$this"),
            ("function", "helper"),
            ("variable", "$x"),
        ],
    );
}

#[test]
fn css_distinguishes_selectors_values_and_custom_properties() {
    let source = ".card > #main:hover::before, li[href] { --gap: 4px; color: red; \
                  margin: var(--gap) 2em; -webkit-box-flex: 1; }\n";
    assert_painted(
        "css",
        source,
        &[
            ("attribute.class", ".card"),
            ("punctuation.delimiter", ">"),
            ("attribute.id", "#main"),
            ("attribute.pseudo", ":hover"),
            ("attribute.pseudo", "::before"),
            ("tag", "li"),
            ("punctuation.special", "["),
            ("attribute", "href"),
            ("variable", "--gap"),
            ("type", "px"),
            ("property", "color"),
            ("constant", "red"),
            ("function", "var"),
            ("property.vendored", "-webkit-box-flex"),
        ],
    );
}

#[test]
fn scss_distinguishes_mixins_functions_and_selectors() {
    let source = "\
@mixin rounded($r) { border-radius: $r; }
.card { @include rounded(2px); &:hover { color: darken($c, 10%); } @if $a == 1 { top: 0; } }
%base { width: 1px; }
";
    assert_painted(
        "scss",
        source,
        &[
            ("keyword", "@mixin"),
            ("function.mixin", "rounded"),
            ("variable", "$r"),
            ("property", "border-radius"),
            ("attribute.class", ".card"),
            ("keyword.import", "@include"),
            ("tag", "&"),
            ("attribute.pseudo", ":hover"),
            ("function", "darken"),
            ("keyword.conditional", "@if"),
            ("operator", "=="),
            ("attribute.class", "%base"),
        ],
    );
}

#[test]
fn html_distinguishes_doctype_attribute_values_and_entities() {
    let source = "<!DOCTYPE html>\n<a href=\"x\" hidden>A &amp; B</a>\n";
    assert_painted(
        "html",
        source,
        &[
            ("punctuation.bracket", "<!"),
            ("tag", "DOCTYPE"),
            ("constant", "html"),
            ("tag", "a"),
            ("attribute", "href"),
            ("string", "\"x\""),
            ("attribute", "hidden"),
            ("constant.character.entity", "&amp;"),
        ],
    );
}

#[test]
fn yaml_distinguishes_escapes_block_scalars_and_document_markers() {
    let source = "---\nname: \"a\\n\"\nbody: |\n  text\nref: &anchor\n  key: !!str 1\n";
    assert_painted(
        "yaml",
        source,
        &[
            ("punctuation.delimiter", "---"),
            ("property", "name"),
            ("string.escape", "\\n"),
            ("keyword.operator", "|"),
            ("string", "text"),
            ("punctuation.special", "&"),
            ("label", "anchor"),
            ("type", "!!str"),
            ("number", "1"),
        ],
    );
}

#[test]
fn shell_distinguishes_variables_builtins_arguments_and_separators() {
    let source = "\
NAME=world
export DIR=\"$HOME/${NAME}\"
echo -n hello | grep -v foo > out.txt
if [[ -f \"$1\" && $n -gt 1 ]]; then return 1; fi
";
    assert_painted(
        "shell",
        source,
        &[
            ("property", "NAME"),
            ("operator", "="),
            ("string", "world"),
            ("keyword", "export"),
            ("property", "$HOME"),
            ("string", "${"),
            ("function.builtin", "echo"),
            ("constant", "-n"),
            ("string", "hello"),
            ("punctuation.delimiter", "|"),
            ("function", "grep"),
            ("punctuation.delimiter", ">"),
            ("string", "out.txt"),
            ("operator", "-f"),
            ("property", "$1"),
            ("operator", "&&"),
            ("operator", "-gt"),
            ("keyword", "return"),
        ],
    );
}

#[test]
fn hcl_keeps_specific_identifier_captures() {
    let source = "\
resource \"kind\" \"name\" {
  count = length(var.zones)
  tags = { Name = \"x\" }
}
";
    assert_painted(
        "hcl",
        source,
        &[
            ("type", "resource"),
            ("string", "\"kind\""),
            ("property", "count"),
            ("function.call", "length"),
            ("variable", "var"),
            ("property", "zones"),
            ("property", "Name"),
        ],
    );
}
