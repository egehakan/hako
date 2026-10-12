use std::path::{Path, PathBuf};
use std::{env, fs};

/// The directories searched for project-local executables and config files: the formatted file's
/// directory and its ancestors up to the project boundary, nearest first.
#[derive(Debug, Clone)]
pub struct SearchScope {
    dirs: Vec<PathBuf>,
}

impl SearchScope {
    /// Builds the scope for `file`. The boundary is the repository enclosing `project_root`, so a
    /// tool installed at the top of a monorepo is found from a nested project. Without a usable
    /// `project_root` the boundary is the repository enclosing the file, or failing that the
    /// file's own directory, so nothing outside the project is ever consulted.
    pub fn new(file: &Path, project_root: Option<&Path>) -> Self {
        let Some(file_dir) = file.parent().filter(|dir| dir.is_absolute()) else {
            return Self { dirs: Vec::new() };
        };
        let project_root = project_root.filter(|root| file_dir.starts_with(root));
        let start = project_root.unwrap_or(file_dir);
        let boundary = enclosing_repository(start).unwrap_or(start);

        let mut dirs = Vec::new();
        for dir in file_dir.ancestors() {
            dirs.push(dir.to_path_buf());
            if dir == boundary {
                break;
            }
        }
        Self { dirs }
    }

    pub fn dirs(&self) -> impl Iterator<Item = &Path> {
        self.dirs.iter().map(PathBuf::as_path)
    }

    /// The outermost directory of the scope.
    pub fn root(&self) -> Option<&Path> {
        self.dirs.last().map(PathBuf::as_path)
    }

    /// The nearest file matching one of `names`.
    pub fn find_file(&self, names: &[&str]) -> Option<PathBuf> {
        self.find(names, |path| path.is_file())
    }

    /// The nearest executable at one of the `relative_paths`.
    pub fn find_executable(&self, relative_paths: &[&str]) -> Option<PathBuf> {
        self.find(relative_paths, is_executable)
    }

    fn find(&self, relative_paths: &[&str], matches: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        self.dirs().find_map(|dir| {
            relative_paths
                .iter()
                .map(|relative| dir.join(relative))
                .find(|candidate| matches(candidate))
        })
    }
}

fn enclosing_repository(dir: &Path) -> Option<&Path> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
}

/// Finds `name` in the directories of `path_env`, a `PATH`-style list. The process's own `PATH`
/// is deliberately not consulted: a GUI app inherits a minimal one that lacks the user's tools.
pub fn find_on_path(name: &str, path_env: Option<&str>) -> Option<PathBuf> {
    env::split_paths(path_env?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// The tests drive stand-in formatters written as shell scripts.
#[cfg(all(test, unix))]
#[path = "discovery_tests.rs"]
mod tests;
