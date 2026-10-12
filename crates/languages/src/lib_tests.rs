use std::path::Path;

use warp_util::standardized_path::StandardizedPath;

use crate::{
    SUPPORTED_LANGUAGES, language_by_filename, language_by_local_filename,
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
