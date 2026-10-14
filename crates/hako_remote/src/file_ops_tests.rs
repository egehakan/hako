use std::fmt::Debug;
use std::os::unix::fs::{PermissionsExt as _, symlink};

use tempfile::TempDir;

use super::*;

/// A temporary directory and its path as the protocol writes paths, without links on the way.
fn tree() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = fs::canonicalize(directory.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    (directory, path)
}

fn code<T: Debug>(result: Result<T, RpcError>) -> ErrorCode {
    result.unwrap_err().code
}

fn write(path: &str, content: &str) {
    fs::write(path, content).unwrap();
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap()
}

fn names(directory: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn mode(path: &str) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn copy_params(
    sources: &[&str],
    destination: &str,
    on_conflict: Option<FsOnConflict>,
) -> FsCopyParams {
    FsCopyParams {
        sources: sources.iter().map(|source| (*source).to_owned()).collect(),
        destination: destination.to_owned(),
        on_conflict,
    }
}

fn run(
    kind: CopyKind,
    sources: &[&str],
    destination: &str,
    on_conflict: Option<FsOnConflict>,
) -> Vec<FsItemResult> {
    copy_or_move(
        &copy_params(sources, destination, on_conflict),
        kind,
        &Deadline::from_now(),
    )
    .unwrap()
    .results
}

fn placed(result: &FsItemResult) -> &str {
    assert_eq!(result.error, None, "{result:?}");
    result.path.as_deref().unwrap()
}

fn error_code(result: &FsItemResult) -> ErrorCode {
    assert_eq!(result.path, None);
    result.error.as_ref().unwrap().code
}

#[test]
fn describes_an_entry_without_following_a_link() {
    let (_directory, root) = tree();
    write(&format!("{root}/a.txt"), "hello");
    fs::create_dir(format!("{root}/sub")).unwrap();
    symlink("a.txt", format!("{root}/link")).unwrap();

    let file = stat(&format!("{root}/a.txt")).unwrap();
    assert_eq!(
        (file.kind, file.size, file.target),
        (FsEntryKind::File, 5, None)
    );
    assert_eq!(stat(&format!("{root}/sub")).unwrap().size, 0);
    let link = stat(&format!("{root}/link")).unwrap();
    assert_eq!(link.kind, FsEntryKind::Symlink);
    assert_eq!(link.target.as_deref(), Some("a.txt"));

    assert_eq!(code(stat("?")), ErrorCode::BadRequest);
    assert_eq!(code(stat(&format!("{root}/missing"))), ErrorCode::NotFound);
}

#[test]
fn creates_an_empty_file_only_where_nothing_is() {
    let (_directory, root) = tree();
    let file = format!("{root}/new.txt");
    create_file(&file).unwrap();
    assert_eq!(read(&file), "");

    assert_eq!(code(create_file(&file)), ErrorCode::Conflict);
    assert_eq!(
        code(create_file(&format!("{root}/no/new.txt"))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(create_file(&format!("{file}/inner.txt"))),
        ErrorCode::NotADirectory
    );
    let long = format!("{root}/{}", "a".repeat(256));
    assert_eq!(code(create_file(&long)), ErrorCode::BadRequest);
}

#[test]
fn renames_within_the_directory_and_never_over_another_entry() {
    let (_directory, root) = tree();
    write(&format!("{root}/a.txt"), "a");
    write(&format!("{root}/b.txt"), "b");
    let home = Path::new("/nowhere");
    let params = |path: &str, name: &str| FsRenameParams {
        path: path.to_owned(),
        name: name.to_owned(),
    };

    let renamed = rename(&params(&format!("{root}/a.txt"), "c.txt"), home).unwrap();
    assert_eq!(renamed.path, format!("{root}/c.txt"));
    assert_eq!(names(&root), ["b.txt", "c.txt"]);

    assert_eq!(
        code(rename(&params(&format!("{root}/c.txt"), "b.txt"), home)),
        ErrorCode::Conflict
    );
    assert_eq!(read(&format!("{root}/b.txt")), "b");
    for name in ["", ".", "..", "a/b", "a\0b"] {
        assert_eq!(
            code(rename(&params(&format!("{root}/c.txt"), name), home)),
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        code(rename(&params(&format!("{root}/missing"), "d"), home)),
        ErrorCode::NotFound
    );
}

#[test]
fn renames_to_a_change_of_case_of_the_same_name() {
    let (_directory, root) = tree();
    write(&format!("{root}/readme.md"), "r");
    let renamed = rename(
        &FsRenameParams {
            path: format!("{root}/readme.md"),
            name: "README.md".to_owned(),
        },
        Path::new("/nowhere"),
    )
    .unwrap();
    assert_eq!(renamed.path, format!("{root}/README.md"));
    assert_eq!(names(&root), ["README.md"]);
}

#[test]
fn never_renames_the_root_or_the_home_directory() {
    let (_directory, root) = tree();
    let home = format!("{root}/home");
    fs::create_dir(&home).unwrap();
    for path in ["/", home.as_str()] {
        let params = FsRenameParams {
            path: path.to_owned(),
            name: "x".to_owned(),
        };
        assert_eq!(
            code(rename(&params, Path::new(&home))),
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn decides_what_happens_when_a_name_is_taken() {
    use FsOnConflict::{Fail, Rename, Replace};
    assert_eq!(placement(None, false, false, Fail), Ok(Placement::Free));
    assert!(placement(Some(false), false, false, Fail).is_err());
    assert_eq!(
        placement(Some(true), true, false, Rename),
        Ok(Placement::KeepBoth)
    );
    assert_eq!(
        placement(Some(false), false, true, Rename),
        Ok(Placement::KeepBoth)
    );
    assert_eq!(
        placement(Some(false), false, false, Replace),
        Ok(Placement::Replace)
    );
    // Only a file or a link replaces a file or a link, and nothing replaces itself.
    assert!(placement(Some(true), false, false, Replace).is_err());
    assert!(placement(Some(false), true, false, Replace).is_err());
    assert!(placement(Some(false), false, true, Replace).is_err());
}

/// A directory with a nested file, a read-only file, a read-only directory and a link.
fn sample_tree(root: &str) -> String {
    let source = format!("{root}/source");
    fs::create_dir_all(format!("{source}/nested/locked")).unwrap();
    write(&format!("{source}/nested/file.txt"), "nested");
    write(&format!("{source}/read-only.txt"), "fixed");
    fs::set_permissions(
        format!("{source}/read-only.txt"),
        Permissions::from_mode(0o444),
    )
    .unwrap();
    write(&format!("{source}/nested/locked/inside.txt"), "inside");
    symlink("nested/file.txt", format!("{source}/link")).unwrap();
    symlink("/nowhere/at/all", format!("{source}/dangling")).unwrap();
    set_raw_times(
        Path::new(&format!("{source}/nested/file.txt")),
        (1_000_000_000, 0),
        (1_000_000_000, 0),
    )
    .unwrap();
    fs::set_permissions(
        format!("{source}/nested/locked"),
        Permissions::from_mode(0o555),
    )
    .unwrap();
    source
}

/// Checks that `copy` is like `source` in contents, permissions, times and links.
fn assert_same_tree(source: &str, copy: &str) {
    assert_eq!(read(&format!("{copy}/nested/file.txt")), "nested");
    assert_eq!(read(&format!("{copy}/nested/locked/inside.txt")), "inside");
    for path in ["read-only.txt", "nested/locked", "nested/file.txt"] {
        assert_eq!(
            mode(&format!("{copy}/{path}")),
            mode(&format!("{source}/{path}"))
        );
    }
    let modified = |path: String| fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(
        modified(format!("{copy}/nested/file.txt")),
        modified(format!("{source}/nested/file.txt"))
    );
    assert_eq!(
        fs::read_link(format!("{copy}/link")).unwrap(),
        Path::new("nested/file.txt")
    );
    assert_eq!(
        fs::read_link(format!("{copy}/dangling")).unwrap(),
        Path::new("/nowhere/at/all")
    );
}

#[test]
fn copies_a_tree_with_its_permissions_times_and_links() {
    let (_directory, root) = tree();
    let source = sample_tree(&root);
    let into = format!("{root}/into");
    fs::create_dir(&into).unwrap();

    let results = run(CopyKind::Copy, &[&source], &into, None);
    assert_eq!(placed(&results[0]), format!("{into}/source"));
    assert_same_tree(&source, &format!("{into}/source"));
    assert_eq!(names(&into), ["source"]);
    remove_tree(Path::new(&root)).unwrap();
}

#[test]
fn duplicates_in_its_own_directory_only_when_asked_to_keep_both() {
    let (_directory, root) = tree();
    write(&format!("{root}/notes.txt"), "n");
    fs::create_dir(format!("{root}/dir.v1")).unwrap();
    let sources = [format!("{root}/notes.txt"), format!("{root}/dir.v1")];
    let sources: Vec<&str> = sources.iter().map(String::as_str).collect();

    let results = run(CopyKind::Copy, &sources, &root, None);
    assert!(
        results
            .iter()
            .all(|result| error_code(result) == ErrorCode::Conflict)
    );
    let results = run(CopyKind::Copy, &sources, &root, Some(FsOnConflict::Replace));
    assert!(
        results
            .iter()
            .all(|result| error_code(result) == ErrorCode::Conflict)
    );

    let results = run(CopyKind::Copy, &sources, &root, Some(FsOnConflict::Rename));
    assert_eq!(placed(&results[0]), format!("{root}/notes (2).txt"));
    assert_eq!(placed(&results[1]), format!("{root}/dir.v1 (2)"));
    let results = run(
        CopyKind::Copy,
        &sources[..1],
        &root,
        Some(FsOnConflict::Rename),
    );
    assert_eq!(placed(&results[0]), format!("{root}/notes (3).txt"));
}

#[test]
fn replaces_only_a_file_or_a_link_with_a_file_or_a_link() {
    let (_directory, root) = tree();
    let into = format!("{root}/into");
    fs::create_dir_all(format!("{into}/dir")).unwrap();
    write(&format!("{into}/file"), "old");
    write(&format!("{root}/file"), "new");
    write(&format!("{root}/dir"), "a file");
    fs::create_dir(format!("{root}/other")).unwrap();
    fs::create_dir(format!("{root}/other/file")).unwrap();

    let results = run(
        CopyKind::Copy,
        &[
            &format!("{root}/file"),
            &format!("{root}/dir"),
            &format!("{root}/other/file"),
        ],
        &into,
        Some(FsOnConflict::Replace),
    );
    assert_eq!(placed(&results[0]), format!("{into}/file"));
    assert_eq!(read(&format!("{into}/file")), "new");
    assert_eq!(error_code(&results[1]), ErrorCode::Conflict);
    assert_eq!(error_code(&results[2]), ErrorCode::Conflict);
    assert_eq!(names(&into), ["dir", "file"]);
}

#[test]
fn never_puts_a_directory_into_itself_and_goes_on_with_the_other_items() {
    let (_directory, root) = tree();
    let parent = format!("{root}/parent");
    fs::create_dir_all(format!("{parent}/child")).unwrap();
    write(&format!("{root}/file"), "f");

    for kind in [CopyKind::Copy, CopyKind::Move] {
        let results = run(kind, &[&parent, &format!("{root}/missing")], &parent, None);
        assert_eq!(error_code(&results[0]), ErrorCode::BadRequest);
        assert_eq!(error_code(&results[1]), ErrorCode::NotFound);
        let results = run(kind, &[&parent], &format!("{parent}/child"), None);
        assert_eq!(error_code(&results[0]), ErrorCode::BadRequest);
    }
    let results = run(
        CopyKind::Copy,
        &[&format!("{root}/file"), "relative"],
        &parent,
        None,
    );
    assert_eq!(placed(&results[0]), format!("{parent}/file"));
    assert_eq!(error_code(&results[1]), ErrorCode::BadRequest);
    assert_eq!(names(&parent), ["child", "file"]);
}

#[test]
fn moves_by_renaming_and_leaves_an_item_moved_onto_itself_alone() {
    let (_directory, root) = tree();
    let into = format!("{root}/into");
    fs::create_dir(&into).unwrap();
    let source = sample_tree(&root);
    write(&format!("{root}/a.txt"), "a");
    write(&format!("{into}/a.txt"), "taken");

    let results = run(
        CopyKind::Move,
        &[&source, &format!("{root}/a.txt")],
        &into,
        Some(FsOnConflict::Rename),
    );
    assert_eq!(placed(&results[0]), format!("{into}/source"));
    assert_eq!(placed(&results[1]), format!("{into}/a (2).txt"));
    assert_eq!(names(&root), ["into"]);

    let in_place = format!("{into}/a.txt");
    let results = run(CopyKind::Move, &[&in_place], &into, None);
    assert_eq!(placed(&results[0]), in_place);
    assert_eq!(read(&in_place), "taken");
    remove_tree(Path::new(&root)).unwrap();
}

#[test]
fn moves_across_volumes_by_copying_and_then_removing_the_original() {
    let (_directory, root) = tree();
    let source = sample_tree(&root);
    let reference = format!("{root}/reference");
    copy_entry(
        Path::new(&source),
        Path::new(&reference),
        &Deadline::from_now(),
    )
    .unwrap();
    let into_path = format!("{root}/into");
    fs::create_dir(&into_path).unwrap();
    let place = fs::canonicalize(&into_path).unwrap();
    let into = Destination {
        path: &into_path,
        directory: Path::new(&into_path),
        place: &place,
    };

    let result = copy_then_place(
        Path::new(&source),
        &source,
        &into,
        Placing {
            name: "source",
            placement: Placement::Free,
            numbered: &|n| upload_names::numbered_directory("source", n),
        },
        CopyKind::Move,
        &Deadline::from_now(),
    );
    assert_eq!(placed(&result), format!("{into_path}/source"));
    assert!(fs::symlink_metadata(&source).is_err());
    assert_same_tree(&reference, &format!("{into_path}/source"));
    remove_tree(Path::new(&root)).unwrap();
}

#[test]
fn stops_at_the_deadline_and_removes_what_it_half_copied() {
    let (_directory, root) = tree();
    let source = sample_tree(&root);
    write(&format!("{root}/later.txt"), "l");
    let into = format!("{root}/into");
    fs::create_dir(&into).unwrap();

    let results = copy_or_move(
        &copy_params(&[&source, &format!("{root}/later.txt")], &into, None),
        CopyKind::Copy,
        &Deadline::after_checks(4),
    )
    .unwrap()
    .results;
    for result in &results {
        assert_eq!(error_code(result), ErrorCode::Failed);
        assert_eq!(
            result.error.as_ref().unwrap().message,
            "Stopped after 240 seconds"
        );
    }
    assert!(names(&into).is_empty());
    remove_tree(Path::new(&root)).unwrap();
}

#[test]
fn refuses_a_destination_that_is_not_a_directory_it_may_write_to() {
    let (_directory, root) = tree();
    write(&format!("{root}/file"), "f");
    let locked = format!("{root}/locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, Permissions::from_mode(0o555)).unwrap();
    let copy = |destination: &str, sources: &[&str]| {
        copy_or_move(
            &copy_params(sources, destination, None),
            CopyKind::Copy,
            &Deadline::from_now(),
        )
    };
    let file = format!("{root}/file");
    assert_eq!(
        code(copy(&format!("{root}/missing"), &[&file])),
        ErrorCode::NotFound
    );
    assert_eq!(code(copy(&file, &[&file])), ErrorCode::NotADirectory);
    assert_eq!(code(copy(&locked, &[&file])), ErrorCode::PermissionDenied);
    assert_eq!(code(copy(&root, &[])), ErrorCode::BadRequest);
    let many = vec![file.as_str(); MAX_ITEMS + 1];
    assert_eq!(code(copy(&root, &many)), ErrorCode::BadRequest);
    remove_tree(Path::new(&root)).unwrap();
}

#[test]
fn trashes_each_item_but_never_the_home_directory_the_root_or_a_folder() {
    let (_directory, root) = tree();
    let home = format!("{root}/home");
    let folder = format!("{home}/project");
    fs::create_dir_all(&folder).unwrap();
    write(&format!("{folder}/a.txt"), "a");
    let trash_directory = format!("{root}/trash");
    fs::create_dir(&trash_directory).unwrap();
    let move_to_trash = |path: &Path| -> Result<PathBuf, String> {
        let in_trash = Path::new(&trash_directory).join(path.file_name().unwrap());
        fs::rename(path, &in_trash).map_err(|error| error.to_string())?;
        Ok(in_trash)
    };

    let paths = [
        format!("{folder}/a.txt"),
        format!("{folder}/missing"),
        "/".to_owned(),
        home.clone(),
        folder.clone(),
        format!("{home}/../home/project"),
    ];
    let results = trash(
        &FsTrashParams {
            paths: paths.to_vec(),
        },
        Path::new(&home),
        &[folder.clone()],
        move_to_trash,
    )
    .unwrap()
    .results;
    assert_eq!(placed(&results[0]), format!("{trash_directory}/a.txt"));
    assert_eq!(error_code(&results[1]), ErrorCode::NotFound);
    for result in &results[2..] {
        assert_eq!(error_code(result), ErrorCode::BadRequest);
    }
    assert!(fs::metadata(&folder).is_ok());
}
