use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use arborium::tree_sitter::{Language as ParserGrammar, Query};
use lazy_static::lazy_static;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use warp_editor::content::text::IndentUnit;
use warp_util::standardized_path::StandardizedPath;

#[derive(RustEmbed)]
#[folder = "grammars"]
struct Grammars;

lazy_static! {
    static ref LANGUAGE_REGISTRY: LanguageRegistry = LanguageRegistry::new();
    /// Every capture name in the highlight queries of the supported languages, along with the
    /// names enclosing it (`function` for `function.method`).
    static ref HIGHLIGHT_CAPTURES: HashSet<String> = SUPPORTED_LANGUAGES
        .iter()
        .filter_map(|lang| highlight_query_source(lang))
        .flat_map(|query| query_capture_names(&query))
        .flat_map(|name| {
            let enclosing: Vec<_> = name
                .match_indices('.')
                .map(|(end, _)| name[..end].to_string())
                .collect();
            enclosing.into_iter().chain([name])
        })
        .collect();
}

pub const SUPPORTED_LANGUAGES: [&str; 43] = [
    "rust",
    "golang",
    "yaml",
    "python",
    "javascript",
    "jsx",
    "typescript",
    "tsx",
    "java",
    "cpp",
    "shell",
    "csharp",
    "html",
    "css",
    "c",
    "json",
    "jq",
    "hcl",
    "lua",
    "ruby",
    "php",
    "toml",
    "swift",
    "kotlin",
    "scala",
    "powershell",
    "elixir",
    "sql",
    "starlark",
    "objective-c",
    "xml",
    "vue",
    "dockerfile",
    "nix",
    "markdown",
    "scss",
    "svelte",
    "typst",
    "ini",
    "diff",
    "make",
    "latex",
    "prisma",
];

/// Registry that holds all of the supported languages.
pub struct LanguageRegistry {
    /// List of languages we support mapped from their display name. They are hold in Arc so they could be shared
    /// between different editors.
    languages: Mutex<HashMap<String, Arc<Language>>>,
}

impl LanguageRegistry {
    fn new() -> Self {
        Self {
            languages: Mutex::new(HashMap::new()),
        }
    }

    pub fn language_by_name(&self, name: &str) -> Option<Arc<Language>> {
        if !SUPPORTED_LANGUAGES.contains(&name) {
            return None;
        }

        let mut languages = self.languages.lock().expect("Mutex should not be poisoned");

        if let Some(lang) = languages.get(name) {
            return Some(lang.clone());
        }

        let language = Arc::new(load_language(name)?);
        languages.insert(name.to_string(), language.clone());
        Some(language)
    }
}

/// Find the corresponding language entry by a standardized filename.
pub fn language_by_filename(path: &StandardizedPath) -> Option<Arc<Language>> {
    language_by_filename_parts(path.file_name(), path.extension())
}

/// Find the corresponding language entry by a local filesystem filename.
pub fn language_by_local_filename(path: &Path) -> Option<Arc<Language>> {
    language_by_filename_parts(
        path.file_name().and_then(|file_name| file_name.to_str()),
        path.extension().and_then(|extension| extension.to_str()),
    )
}

/// Normalizes common language-name aliases to their canonical internal names.
/// For example, "go" -> "golang", "bash" -> "shell", "md" -> "markdown".
fn normalize_language_name(name: &str) -> &str {
    match name {
        "go" => "golang",
        "bash" | "sh" | "zsh" | "dotenv" | "env" => "shell",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "py" | "python3" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "cs" | "c#" => "csharp",
        "c++" => "cpp",
        "objc" | "objective_c" => "objective-c",
        "terraform" | "tf" => "hcl",
        "kt" => "kotlin",
        "docker" | "containerfile" => "dockerfile",
        "md" | "mdx" => "markdown",
        "yml" => "yaml",
        "jsonc" | "jsonl" => "json",
        "postgres" | "postgresql" | "pgsql" | "plpgsql" | "psql" => "sql",
        "handlebars" | "hbs" | "blade" => "html",
        "svg" | "plist" => "xml",
        "typ" => "typst",
        "patch" => "diff",
        "makefile" => "make",
        "tex" => "latex",
        other => other,
    }
}

pub fn language_by_name(name: &str) -> Option<Arc<Language>> {
    let normalized = normalize_language_name(name);
    LANGUAGE_REGISTRY.language_by_name(normalized)
}

fn language_by_filename_parts(
    filename: Option<&str>,
    extension: Option<&str>,
) -> Option<Arc<Language>> {
    language_by_name(language_name_for_filename_parts(filename, extension)?)
}

/// Suffixes appended to a real filename to mark a checked-in template of it, as in
/// `.env.example` or `phpunit.xml.dist`.
const TEMPLATE_SUFFIXES: [&str; 3] = [".example", ".sample", ".dist"];

/// Resolves a file to an internal language name. Whole filenames take precedence over
/// extensions, which are retried in lowercase so `README.MD` and `api.Dockerfile` resolve.
fn language_name_for_filename_parts(
    filename: Option<&str>,
    extension: Option<&str>,
) -> Option<&'static str> {
    let template_of = filename.and_then(|filename| {
        TEMPLATE_SUFFIXES
            .iter()
            .find_map(|suffix| filename.strip_suffix(suffix))
    });
    if let Some(template_of) = template_of {
        let extension = Path::new(template_of)
            .extension()
            .and_then(|extension| extension.to_str());
        if let Some(name) = language_name_for_filename_parts(Some(template_of), extension) {
            return Some(name);
        }
    }

    filename.and_then(language_name_for_filename).or_else(|| {
        let extension = extension?;
        language_name_for_extension(extension)
            .or_else(|| language_name_for_extension(&extension.to_ascii_lowercase()))
    })
}

/// Resolves files that are identified by their whole name, a name prefix, or a compound
/// extension rather than by their final extension.
fn language_name_for_filename(filename: &str) -> Option<&'static str> {
    let name = match filename {
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_login" | ".bash_logout"
        | ".profile" => "shell",
        ".zshrc" | ".zsh_profile" | ".zprofile" | ".zshenv" | ".zlogin" | ".zlogout" => "shell",
        // Dotenv-style files are `KEY=value` lines with `#` comments, which is valid shell.
        ".envrc" | ".dev.vars" | ".flaskenv" | "env.example" | "env.sample" => "shell",
        "BUILD" | "WORKSPACE" => "starlark",
        "Dockerfile" | "Containerfile" | "dockerfile" | "containerfile" => "dockerfile",
        "Gemfile" | "Rakefile" | "Podfile" | "Fastfile" | "Brewfile" | "Vagrantfile" => "ruby",
        "Cargo.lock" | "poetry.lock" | "uv.lock" | "Pipfile" => "toml",
        "composer.lock" | "bun.lock" | "flake.lock" | "Pipfile.lock" => "json",
        // These rc files may also be written in YAML, but JSON (often with comments, which the
        // YAML grammar cannot parse) is the overwhelmingly common form.
        ".babelrc" | ".swcrc" | ".prettierrc" | ".eslintrc" | ".stylelintrc" => "json",
        ".clang-format" | ".yamllint" => "yaml",
        ".npmrc" | ".editorconfig" => "ini",
        "Makefile" | "makefile" | "GNUmakefile" => "make",
        _ => {
            if filename.starts_with("Dockerfile.") || filename.starts_with("Containerfile.") {
                "dockerfile"
            } else if filename == ".env" || filename.starts_with(".env.") {
                "shell"
            } else if filename.to_ascii_lowercase().ends_with(".blade.php") {
                // Blade templates are mostly markup, and the PHP grammar leaves everything
                // outside `<?php` tags unhighlighted.
                "html"
            } else {
                return None;
            }
        }
    };
    Some(name)
}

fn language_name_for_extension(extension: &str) -> Option<&'static str> {
    let name = match extension {
        "rs" => "rust",
        "go" => "golang",
        "yml" | "yaml" => "yaml",
        "py" | "py3" | "pyw" | "pyi" => "python",
        "js" | "cjs" | "mjs" => "javascript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "ts" | "cts" | "mts" => "typescript",
        "java" | "groovy" | "gvy" | "gy" | "gsh" => "java",
        "cpp" | "cxx" | "cc" | "h" | "hh" | "hpp" | "hxx" | "H" | "h++" => "cpp",
        "sh" | "zsh" | "bash" | "command" | "env" => "shell",
        "cs" => "csharp",
        // Handlebars and Mustache templates are markup with `{{ }}` placeholders.
        "html" | "htm" | "hbs" | "handlebars" | "mustache" => "html",
        "css" => "css",
        "c" => "c",
        "json" | "jsonc" | "jsonl" => "json",
        "jq" => "jq",
        "tf" | "hcl" | "tfvars" => "hcl",
        "lua" => "lua",
        "nix" => "nix",
        "rb" | "rake" | "gemspec" | "podspec" => "ruby",
        "php" | "phtml" => "php",
        "toml" => "toml",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" | "sbt" | "sc" => "scala",
        "ps1" | "pwsh" => "powershell",
        "ex" | "exs" => "elixir",
        "sql" | "psql" | "pgsql" | "plpgsql" | "postgres" => "sql",
        "bzl" | "bazel" => "starlark",
        "m" | "mm" => "objective-c",
        "xml" | "svg" | "plist" | "xsd" | "xsl" | "drawio" => "xml",
        "vue" => "vue",
        "dockerfile" => "dockerfile",
        "md" | "markdown" | "mdx" | "mdc" => "markdown",
        "scss" => "scss",
        "svelte" => "svelte",
        "typ" => "typst",
        "ini" | "cfg" | "service" => "ini",
        "diff" | "patch" => "diff",
        "mk" | "mak" => "make",
        "tex" | "sty" | "cls" | "ltx" => "latex",
        "prisma" => "prisma",
        _ => return None,
    };
    Some(name)
}

/// Captures the language-specific parser grammar and queries for syntax features like highlighting and
/// bracket pairing. In the future, this will also be the entry point for LSP.
pub struct Language {
    /// Tree-sitter parser grammar.
    pub grammar: ParserGrammar,
    /// Query for syntax highlighting.
    pub highlight_query: Query,
    /// Query for auto indent.
    pub indents_query: Option<Query>,
    /// Unit for each indent action.
    pub indent_unit: IndentUnit,
    /// Comment prefix.
    pub comment_prefix: Option<String>,
    /// Language-specific bracket pairs.
    pub bracket_pairs: Vec<(char, char)>,
    /// Query for parsing symbols.
    pub symbols_query: Option<Query>,
    /// Display name for the language.
    pub display_name: String,
    /// Name the language is registered under, as listed in [`SUPPORTED_LANGUAGES`].
    pub name: String,
}

impl Language {
    /// Returns the display name of the language.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct LanguageConfig {
    display_name: String,
    indent_unit: IndentUnit,
    comment_prefix: Option<String>,
    #[serde(default)]
    brackets: Vec<BracketPair>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BracketPair {
    start: String,
    end: String,
}

/// Map our internal language name to the canonical arborium language name.
fn to_arborium_name(lang: &str) -> &str {
    match lang {
        "golang" => "go",
        "shell" => "bash",
        "csharp" => "c-sharp",
        "jsx" => "javascript",
        "objective-c" => "objc",
        "sql" => "sql",
        other => other,
    }
}

/// Get the bundled highlight query from arborium for a given language.
fn get_arborium_highlight_query(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some(arborium::lang_rust::HIGHLIGHTS_QUERY),
        "golang" => Some(arborium::lang_go::HIGHLIGHTS_QUERY),
        "yaml" => Some(arborium::lang_yaml::HIGHLIGHTS_QUERY),
        "python" => Some(arborium::lang_python::HIGHLIGHTS_QUERY),
        "javascript" => Some(arborium::lang_javascript::HIGHLIGHTS_QUERY),
        "jsx" => Some(arborium::lang_javascript::HIGHLIGHTS_QUERY),
        "typescript" => Some(&arborium::lang_typescript::HIGHLIGHTS_QUERY),
        "tsx" => Some(&arborium::lang_tsx::HIGHLIGHTS_QUERY),
        "java" => Some(arborium::lang_java::HIGHLIGHTS_QUERY),
        "cpp" => Some(&arborium::lang_cpp::HIGHLIGHTS_QUERY),
        "shell" => Some(arborium::lang_bash::HIGHLIGHTS_QUERY),
        "csharp" => Some(arborium::lang_c_sharp::HIGHLIGHTS_QUERY),
        "html" => Some(arborium::lang_html::HIGHLIGHTS_QUERY),
        "css" => Some(arborium::lang_css::HIGHLIGHTS_QUERY),
        "c" => Some(arborium::lang_c::HIGHLIGHTS_QUERY),
        "json" => Some(arborium::lang_json::HIGHLIGHTS_QUERY),
        "jq" => Some(arborium::lang_jq::HIGHLIGHTS_QUERY),
        "hcl" => Some(arborium::lang_hcl::HIGHLIGHTS_QUERY),
        "lua" => Some(arborium::lang_lua::HIGHLIGHTS_QUERY),
        "nix" => Some(arborium::lang_nix::HIGHLIGHTS_QUERY),
        "ruby" => Some(arborium::lang_ruby::HIGHLIGHTS_QUERY),
        "php" => Some(arborium::lang_php::HIGHLIGHTS_QUERY),
        "toml" => Some(arborium::lang_toml::HIGHLIGHTS_QUERY),
        "swift" => Some(arborium::lang_swift::HIGHLIGHTS_QUERY),
        "kotlin" => Some(arborium::lang_kotlin::HIGHLIGHTS_QUERY),
        "scala" => Some(arborium::lang_scala::HIGHLIGHTS_QUERY),
        "powershell" => Some(arborium::lang_powershell::HIGHLIGHTS_QUERY),
        "elixir" => Some(arborium::lang_elixir::HIGHLIGHTS_QUERY),
        "sql" => Some(arborium::lang_sql::HIGHLIGHTS_QUERY),
        "starlark" => Some(arborium::lang_starlark::HIGHLIGHTS_QUERY),
        "objective-c" => Some(&arborium::lang_objc::HIGHLIGHTS_QUERY),
        "xml" => Some(arborium::lang_xml::HIGHLIGHTS_QUERY),
        "vue" => Some(&arborium::lang_vue::HIGHLIGHTS_QUERY),
        "dockerfile" => Some(arborium::lang_dockerfile::HIGHLIGHTS_QUERY),
        "markdown" => Some(arborium::lang_markdown::HIGHLIGHTS_QUERY),
        "scss" => Some(&arborium::lang_scss::HIGHLIGHTS_QUERY),
        "svelte" => Some(&arborium::lang_svelte::HIGHLIGHTS_QUERY),
        "typst" => Some(arborium::lang_typst::HIGHLIGHTS_QUERY),
        "ini" => Some(arborium::lang_ini::HIGHLIGHTS_QUERY),
        "diff" => Some(arborium::lang_diff::HIGHLIGHTS_QUERY),
        _ => None,
    }
}

/// Returns the highlight query source for a language: arborium's bundled query followed by the
/// language's optional `highlights.scm`, whose patterns take precedence because the editor
/// applies captures in pattern order.
fn highlight_query_source(lang: &str) -> Option<Cow<'static, str>> {
    let bundled = get_arborium_highlight_query(lang);
    let local = load_text(&[lang, "highlights.scm"].join("\\"));
    match (bundled, local) {
        (Some(bundled), Some(local)) => Some(Cow::Owned(format!("{bundled}\n{local}"))),
        (Some(bundled), None) => Some(Cow::Borrowed(bundled)),
        (None, Some(local)) => Some(local),
        (None, None) => None,
    }
}

/// Returns whether the highlight query of any supported language captures `name` or a capture
/// nested under it, as `function` is for a query that only captures `function.method`.
pub fn is_highlight_capture(name: &str) -> bool {
    HIGHLIGHT_CAPTURES.contains(name)
}

/// Returns the capture names a query source uses, skipping those inside strings and comments
/// and the `_`-prefixed ones that only serve the query's own predicates.
fn query_capture_names(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            ';' => {
                chars.by_ref().find(|c| *c == '\n');
            }
            '@' => {
                let mut name = String::new();
                while let Some(c) = chars
                    .next_if(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '?' | '!'))
                {
                    name.push(c);
                }
                if !name.is_empty() && !name.starts_with('_') {
                    names.push(name);
                }
            }
            _ => {}
        }
    }
    names
}

/// Returns the tree-sitter grammar for a language, from a standalone grammar crate for the few
/// languages arborium does not bundle.
fn grammar_for(lang: &str) -> Option<ParserGrammar> {
    match lang {
        "make" => Some(tree_sitter_make::LANGUAGE.into()),
        "latex" => Some(codebook_tree_sitter_latex::LANGUAGE.into()),
        "prisma" => Some(tree_sitter_prisma_io::LANGUAGE.into()),
        _ => arborium::get_language(to_arborium_name(lang)),
    }
}

fn load_language(lang: &str) -> Option<Language> {
    let grammar = grammar_for(lang)?;

    let config_path = [lang, "config.yaml"].join("\\");
    let config = load_yaml(&config_path);
    let indent_unit = config.indent_unit;
    let comment_prefix = config.comment_prefix;
    let bracket_pairs = config
        .brackets
        .into_iter()
        .filter_map(|bracket_pair| {
            let start = bracket_pair.start.chars().next()?;
            let end = bracket_pair.end.chars().next()?;
            Some((start, end))
        })
        .collect();

    let highlight_query_str = highlight_query_source(lang)?;
    let highlight_query = Query::new(&grammar, &highlight_query_str)
        .unwrap_or_else(|err| panic!("highlight query for {lang} should be valid: {err}"));

    let indents_query_path = [lang, "indents.scm"].join("\\");
    let indents_query = load_query(&indents_query_path, &grammar);

    let symbols_query_path = [lang, "identifiers.scm"].join("\\");
    let symbols_query = load_query(&symbols_query_path, &grammar);

    Some(Language {
        highlight_query,
        indents_query,
        grammar,
        indent_unit,
        comment_prefix,
        bracket_pairs,
        symbols_query,
        display_name: config.display_name,
        name: lang.to_string(),
    })
}

fn load_yaml(path: &str) -> LanguageConfig {
    match <Grammars as RustEmbed>::get(path) {
        Some(file) => {
            let config: LanguageConfig =
                serde_yaml::from_slice(&file.data).expect("Unable to deserialize the YAML content");
            config
        }
        None => {
            panic!("Couldn't initiate yaml config from {path}");
        }
    }
}

fn load_text(path: &str) -> Option<Cow<'static, str>> {
    let file = <Grammars as RustEmbed>::get(path)?;
    Some(match file.data {
        Cow::Borrowed(inner) => Cow::Borrowed(std::str::from_utf8(inner).unwrap()),
        Cow::Owned(inner) => Cow::Owned(String::from_utf8(inner).unwrap()),
    })
}

fn load_query(path: &str, grammar: &ParserGrammar) -> Option<Query> {
    let query_content = load_text(path)?;

    Some(
        Query::new(grammar, &query_content)
            .unwrap_or_else(|err| panic!("TSQuery creation should work from {path}: {err}")),
    )
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
