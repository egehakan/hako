use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{
    CreateError, FileOpError, PasteKind, copy_recursively, create_new, duplicate, failure_message,
    free_destination, is_same_or_inside, paste, paste_target_dir, top_level_paths,
};

fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("temp dir is created")
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent is created");
    }
    fs::write(path, content).expect("file is written");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("file is read")
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("folder is read")
        .map(|entry| {
            entry
                .expect("entry is read")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn free_name(dir: &Path, name: &str, is_dir: bool) -> String {
    free_destination(dir, OsStr::new(name), is_dir)
        .file_name()
        .expect("destination has a name")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn test_free_name_is_kept() {
    let dir = temp_dir();

    assert_eq!(free_name(dir.path(), "notes.txt", false), "notes.txt");
    assert_eq!(free_name(dir.path(), "src", true), "src");
}

#[test]
fn test_taken_file_name_gets_numbered_copy_names_before_the_extension() {
    let dir = temp_dir();
    write(&dir.path().join("notes.txt"), "");
    assert_eq!(free_name(dir.path(), "notes.txt", false), "notes copy.txt");

    write(&dir.path().join("notes copy.txt"), "");
    assert_eq!(
        free_name(dir.path(), "notes.txt", false),
        "notes copy 2.txt"
    );

    write(&dir.path().join("notes copy 2.txt"), "");
    assert_eq!(
        free_name(dir.path(), "notes.txt", false),
        "notes copy 3.txt"
    );
}

#[test]
fn test_copy_of_a_copy_continues_the_numbering() {
    let dir = temp_dir();
    write(&dir.path().join("notes copy.txt"), "");
    assert_eq!(
        free_name(dir.path(), "notes copy.txt", false),
        "notes copy 2.txt"
    );

    write(&dir.path().join("notes copy 2.txt"), "");
    assert_eq!(
        free_name(dir.path(), "notes copy 2.txt", false),
        "notes copy 3.txt"
    );
}

#[test]
fn test_copy_names_for_folders_dotfiles_and_compound_extensions() {
    let dir = temp_dir();
    fs::create_dir(dir.path().join("v1.2")).expect("folder is created");
    write(&dir.path().join(".env"), "");
    write(&dir.path().join("Makefile"), "");
    write(&dir.path().join("backup.tar.gz"), "");

    // A folder name has no extension to keep at the end, even when it has a dot in it.
    assert_eq!(free_name(dir.path(), "v1.2", true), "v1.2 copy");
    assert_eq!(free_name(dir.path(), ".env", false), ".env copy");
    assert_eq!(free_name(dir.path(), "Makefile", false), "Makefile copy");
    assert_eq!(
        free_name(dir.path(), "backup.tar.gz", false),
        "backup copy.tar.gz"
    );
}

#[cfg(unix)]
#[test]
fn test_name_taken_by_a_dangling_link_is_not_reused() {
    let dir = temp_dir();
    std::os::unix::fs::symlink("missing", dir.path().join("link")).expect("link is created");

    assert_eq!(free_name(dir.path(), "link", false), "link copy");
}

#[test]
fn test_paste_target_is_the_selected_folder_the_files_folder_or_the_root() {
    let root = Path::new("/repo");

    assert_eq!(
        paste_target_dir(Some((Path::new("/repo/src"), true)), root),
        PathBuf::from("/repo/src")
    );
    assert_eq!(
        paste_target_dir(Some((Path::new("/repo/src/main.rs"), false)), root),
        PathBuf::from("/repo/src")
    );
    assert_eq!(paste_target_dir(None, root), PathBuf::from("/repo"));
}

#[test]
fn test_is_same_or_inside_compares_whole_components() {
    assert!(is_same_or_inside(
        Path::new("/repo/src"),
        Path::new("/repo/src")
    ));
    assert!(is_same_or_inside(
        Path::new("/repo/src/a/b"),
        Path::new("/repo/src")
    ));
    assert!(!is_same_or_inside(
        Path::new("/repo/src2"),
        Path::new("/repo/src")
    ));
    assert!(!is_same_or_inside(
        Path::new("/repo"),
        Path::new("/repo/src")
    ));
}

#[test]
fn test_top_level_paths_drops_nested_and_repeated_paths() {
    let paths = vec![
        PathBuf::from("/repo/src/main.rs"),
        PathBuf::from("/repo/src"),
        PathBuf::from("/repo/README.md"),
        PathBuf::from("/repo/src"),
        PathBuf::from("/repo/src/a/b.rs"),
    ];

    assert_eq!(
        top_level_paths(paths),
        vec![PathBuf::from("/repo/src"), PathBuf::from("/repo/README.md")]
    );
}

#[cfg(unix)]
#[test]
fn test_copy_of_a_nested_folder_keeps_hidden_files_and_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = temp_dir();
    let source = dir.path().join("project");
    write(&source.join("README.md"), "readme");
    write(&source.join(".env"), "SECRET=1");
    write(&source.join(".git/config"), "[core]");
    write(&source.join("src/deep/er/main.rs"), "fn main() {}");
    fs::create_dir(source.join("empty")).expect("folder is created");
    symlink("README.md", source.join("link-to-file")).expect("link is created");
    symlink("src", source.join("link-to-folder")).expect("link is created");
    symlink("nowhere", source.join("dangling")).expect("link is created");
    let script = source.join("run.sh");
    write(&script, "#!/bin/sh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode is set");

    let destination = dir.path().join("copied");
    copy_recursively(&source, &destination).expect("folder is copied");

    assert_eq!(read(&destination.join("README.md")), "readme");
    assert_eq!(read(&destination.join(".env")), "SECRET=1");
    assert_eq!(read(&destination.join(".git/config")), "[core]");
    assert_eq!(
        read(&destination.join("src/deep/er/main.rs")),
        "fn main() {}"
    );
    assert!(destination.join("empty").is_dir());
    for (link, target) in [
        ("link-to-file", "README.md"),
        ("link-to-folder", "src"),
        ("dangling", "nowhere"),
    ] {
        let copied = destination.join(link);
        assert!(
            fs::symlink_metadata(&copied)
                .expect("link is copied")
                .file_type()
                .is_symlink(),
            "{link} is copied as a link"
        );
        assert_eq!(
            fs::read_link(&copied).expect("link is read"),
            PathBuf::from(target)
        );
    }
    let mode = fs::metadata(destination.join("run.sh"))
        .expect("script is copied")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    // The original is left as it was.
    assert_eq!(read(&source.join("src/deep/er/main.rs")), "fn main() {}");
}

#[test]
fn test_paste_copies_several_items_and_never_replaces_what_is_there() {
    let dir = temp_dir();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    write(&from.join("a.txt"), "new a");
    write(&from.join("b.txt"), "new b");
    write(&from.join("pkg/mod.rs"), "new mod");
    write(&to.join("a.txt"), "old a");
    write(&to.join("pkg/mod.rs"), "old mod");

    let outcome = paste(
        &[from.join("a.txt"), from.join("b.txt"), from.join("pkg")],
        &to,
        PasteKind::Copy,
    );

    assert!(outcome.failed.is_empty());
    let destinations: Vec<PathBuf> = outcome
        .placed
        .iter()
        .map(|item| item.destination.clone())
        .collect();
    assert_eq!(
        destinations,
        vec![to.join("a copy.txt"), to.join("b.txt"), to.join("pkg copy")]
    );
    assert_eq!(read(&to.join("a.txt")), "old a");
    assert_eq!(read(&to.join("a copy.txt")), "new a");
    assert_eq!(read(&to.join("b.txt")), "new b");
    assert_eq!(read(&to.join("pkg/mod.rs")), "old mod");
    assert_eq!(read(&to.join("pkg copy/mod.rs")), "new mod");
    assert_eq!(names_in(&from), ["a.txt", "b.txt", "pkg"]);
}

#[test]
fn test_paste_of_a_cut_moves_the_items() {
    let dir = temp_dir();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    write(&from.join("a.txt"), "a");
    write(&from.join("pkg/inner/mod.rs"), "mod");
    write(&from.join("stays.txt"), "stays");
    fs::create_dir(&to).expect("folder is created");

    let outcome = paste(
        &[from.join("a.txt"), from.join("pkg")],
        &to,
        PasteKind::Move,
    );

    assert!(outcome.failed.is_empty());
    assert_eq!(outcome.placed.len(), 2);
    assert_eq!(names_in(&from), ["stays.txt"]);
    assert_eq!(read(&to.join("a.txt")), "a");
    assert_eq!(read(&to.join("pkg/inner/mod.rs")), "mod");
}

#[test]
fn test_move_onto_a_taken_name_keeps_both() {
    let dir = temp_dir();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    write(&from.join("a.txt"), "moved");
    write(&to.join("a.txt"), "already there");

    let outcome = paste(&[from.join("a.txt")], &to, PasteKind::Move);

    assert!(outcome.failed.is_empty());
    assert_eq!(outcome.placed[0].destination, to.join("a copy.txt"));
    assert_eq!(read(&to.join("a.txt")), "already there");
    assert_eq!(read(&to.join("a copy.txt")), "moved");
    assert!(names_in(&from).is_empty());
}

#[test]
fn test_move_into_the_folder_an_item_is_already_in_leaves_it_alone() {
    let dir = temp_dir();
    let file = dir.path().join("a.txt");
    write(&file, "a");

    let outcome = paste(std::slice::from_ref(&file), dir.path(), PasteKind::Move);

    assert!(outcome.failed.is_empty());
    assert_eq!(outcome.placed[0].destination, file);
    assert_eq!(names_in(dir.path()), ["a.txt"]);
}

#[test]
fn test_copy_into_the_folder_an_item_is_already_in_makes_a_copy() {
    let dir = temp_dir();
    let file = dir.path().join("a.txt");
    write(&file, "a");

    let outcome = paste(std::slice::from_ref(&file), dir.path(), PasteKind::Copy);

    assert!(outcome.failed.is_empty());
    assert_eq!(names_in(dir.path()), ["a copy.txt", "a.txt"]);
}

#[test]
fn test_folder_is_not_pasted_into_itself_or_its_descendants() {
    let dir = temp_dir();
    let folder = dir.path().join("pkg");
    write(&folder.join("inner/deep/mod.rs"), "mod");

    for (target, kind) in [
        (folder.clone(), PasteKind::Copy),
        (folder.join("inner/deep"), PasteKind::Copy),
        (folder.clone(), PasteKind::Move),
        (folder.join("inner"), PasteKind::Move),
    ] {
        let outcome = paste(std::slice::from_ref(&folder), &target, kind);

        assert!(outcome.placed.is_empty());
        let expected_error = match kind {
            PasteKind::Copy => matches!(outcome.failed[0].error, FileOpError::CopyIntoItself),
            PasteKind::Move => matches!(outcome.failed[0].error, FileOpError::MoveIntoItself),
        };
        assert!(
            expected_error,
            "unexpected error {:?}",
            outcome.failed[0].error
        );
    }
    assert_eq!(names_in(&folder), ["inner"]);
    assert_eq!(names_in(&folder.join("inner/deep")), ["mod.rs"]);
}

#[cfg(unix)]
#[test]
fn test_folder_is_not_pasted_into_itself_through_a_link() {
    let dir = temp_dir();
    let folder = dir.path().join("pkg");
    write(&folder.join("inner/mod.rs"), "mod");
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(folder.join("inner"), &alias).expect("link is created");

    let outcome = paste(std::slice::from_ref(&folder), &alias, PasteKind::Copy);

    assert!(outcome.placed.is_empty());
    assert!(matches!(
        outcome.failed[0].error,
        FileOpError::CopyIntoItself
    ));
}

#[test]
fn test_missing_item_fails_without_stopping_the_others() {
    let dir = temp_dir();
    let to = dir.path().join("to");
    fs::create_dir(&to).expect("folder is created");
    let present = dir.path().join("present.txt");
    write(&present, "here");
    let missing = dir.path().join("missing.txt");

    let outcome = paste(&[missing.clone(), present], &to, PasteKind::Copy);

    assert_eq!(outcome.placed.len(), 1);
    assert_eq!(read(&to.join("present.txt")), "here");
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].source, missing);
    assert!(matches!(
        outcome.failed[0].error,
        FileOpError::SourceMissing
    ));
    assert_eq!(
        failure_message("copy", &outcome.failed),
        "Couldn’t copy “missing.txt”: it no longer exists"
    );
}

#[test]
fn test_duplicate_copies_each_item_next_to_itself() {
    let dir = temp_dir();
    write(&dir.path().join("a.txt"), "a");
    write(&dir.path().join("pkg/mod.rs"), "mod");
    write(&dir.path().join("pkg/b.txt"), "b");

    let outcome = duplicate(&[
        dir.path().join("a.txt"),
        dir.path().join("pkg"),
        dir.path().join("pkg/b.txt"),
    ]);

    assert!(outcome.failed.is_empty());
    assert_eq!(
        names_in(dir.path()),
        ["a copy.txt", "a.txt", "pkg", "pkg copy"]
    );
    assert_eq!(read(&dir.path().join("pkg copy/mod.rs")), "mod");
    assert_eq!(
        names_in(&dir.path().join("pkg")),
        ["b copy.txt", "b.txt", "mod.rs"]
    );
}

#[test]
fn test_new_file_and_folder_are_created_empty() {
    let dir = temp_dir();

    let file = create_new(dir.path(), "notes.txt", false).expect("file is created");
    assert_eq!(file.path, dir.path().join("notes.txt"));
    assert_eq!(file.created_root, file.path);
    assert_eq!(read(&file.path), "");

    let folder = create_new(dir.path(), " src ", true).expect("folder is created");
    assert_eq!(folder.path, dir.path().join("src"));
    assert!(folder.path.is_dir());
}

#[test]
fn test_nested_name_creates_the_folders_on_the_way() {
    let dir = temp_dir();
    fs::create_dir(dir.path().join("components")).expect("folder is created");

    let file = create_new(dir.path(), "components/forms/fields/Login.tsx", false)
        .expect("file is created");

    assert_eq!(
        read(&dir.path().join("components/forms/fields/Login.tsx")),
        ""
    );
    // What undoing removes is the first folder that was not there before.
    assert_eq!(file.created_root, dir.path().join("components/forms"));

    let folder = create_new(dir.path(), "a/b/c", true).expect("folder is created");
    assert!(dir.path().join("a/b/c").is_dir());
    assert_eq!(folder.created_root, dir.path().join("a"));
}

#[test]
fn test_taken_empty_and_escaping_names_create_nothing() {
    let dir = temp_dir();
    write(&dir.path().join("inner/taken.txt"), "mine");

    assert!(matches!(
        create_new(&dir.path().join("inner"), "taken.txt", false),
        Err(CreateError::AlreadyExists(name)) if name == "taken.txt"
    ));
    assert!(matches!(
        create_new(dir.path(), "inner", true),
        Err(CreateError::AlreadyExists(_))
    ));
    assert!(matches!(
        create_new(dir.path(), "  ", false),
        Err(CreateError::EmptyName)
    ));
    for name in [
        "../outside.txt",
        "inner/../../outside.txt",
        "/tmp/absolute.txt",
        "./here.txt",
    ] {
        assert!(
            matches!(
                create_new(&dir.path().join("inner"), name, false),
                Err(CreateError::InvalidName(_))
            ),
            "{name} is refused"
        );
    }
    // A file in the way of a folder that would have to be created.
    assert!(matches!(
        create_new(dir.path(), "inner/taken.txt/below.txt", false),
        Err(CreateError::Io(_))
    ));

    assert_eq!(read(&dir.path().join("inner/taken.txt")), "mine");
    assert_eq!(names_in(dir.path()), ["inner"]);
    assert_eq!(names_in(&dir.path().join("inner")), ["taken.txt"]);
}
