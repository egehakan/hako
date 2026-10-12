use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{CreatedItem, Fingerprint, MovedItem, Origin, Reversal, Step, TrashedItem, redo, undo};
use crate::code::file_tree::file_ops::{self, PasteKind};
use crate::code::file_tree::trash::{self, TEST_TRASH_DIR};

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

/// Trashes `paths` the way the tree does and returns the step that undoes it.
fn trash_step(paths: &[&Path]) -> Step {
    Step::Trashed(
        paths
            .iter()
            .map(|path| TrashedItem {
                original: path.to_path_buf(),
                in_trash: trash::move_to_trash(path).expect("item is trashed"),
            })
            .collect(),
    )
}

/// Copies `sources` into `target` the way a paste does and returns the step that undoes it.
fn copy_step(sources: &[PathBuf], target: &Path) -> Step {
    let outcome = file_ops::paste(sources, target, PasteKind::Copy);
    assert!(outcome.failed.is_empty());
    Step::Created(
        outcome
            .placed
            .into_iter()
            .map(|item| CreatedItem {
                path: item.destination,
                origin: Origin::CopyOf(item.source),
                fingerprint: item.fingerprint.expect("copy has a fingerprint"),
            })
            .collect(),
    )
}

fn fully_reversed(reversal: Reversal) -> Step {
    assert_eq!(reversal.problem, None);
    assert_eq!(reversal.remaining, None);
    reversal.reversed.expect("step is reversed")
}

#[test]
fn test_undo_of_a_delete_puts_the_items_back_and_redo_trashes_them_again() {
    let dir = temp_dir();
    let file = dir.path().join("a.txt");
    let folder = dir.path().join("pkg");
    write(&file, "a");
    write(&folder.join("inner/mod.rs"), "mod");

    let step = trash_step(&[&file, &folder]);
    assert!(!file.exists() && !folder.exists());

    let step = fully_reversed(undo(step));
    assert_eq!(read(&file), "a");
    assert_eq!(read(&folder.join("inner/mod.rs")), "mod");

    let step = fully_reversed(redo(step));
    assert!(!file.exists() && !folder.exists());
    assert_eq!(read(&dir.path().join(TEST_TRASH_DIR).join("a.txt")), "a");

    fully_reversed(undo(step));
    assert_eq!(read(&file), "a");
}

#[test]
fn test_undo_of_a_delete_stops_where_the_original_place_is_taken() {
    let dir = temp_dir();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let third = dir.path().join("third.txt");
    for (path, content) in [(&first, "1"), (&second, "2"), (&third, "3")] {
        write(path, content);
    }
    let step = trash_step(&[&first, &second, &third]);
    write(&second, "written since");

    let reversal = undo(step);

    // The first item is back; the one in the way and the ones after it are left in the trash.
    assert_eq!(read(&first), "1");
    assert_eq!(read(&second), "written since");
    assert!(!third.exists());
    let in_trash = dir.path().join(TEST_TRASH_DIR);
    assert_eq!(read(&in_trash.join("second.txt")), "2");
    assert_eq!(read(&in_trash.join("third.txt")), "3");
    assert!(matches!(reversal.reversed, Some(Step::Trashed(items)) if items.len() == 1));
    assert!(matches!(&reversal.remaining, Some(Step::Trashed(items)) if items.len() == 2));
    assert!(
        reversal
            .problem
            .as_deref()
            .is_some_and(|problem| problem.starts_with("“second.txt” can’t be put back")),
        "{:?}",
        reversal.problem
    );

    // Once the place is free again, the rest can be put back.
    fs::remove_file(&second).expect("file is removed");
    fully_reversed(undo(reversal.remaining.expect("items remain")));
    assert_eq!(read(&second), "2");
    assert_eq!(read(&third), "3");
}

#[test]
fn test_undo_of_a_delete_reports_an_item_that_left_the_trash() {
    let dir = temp_dir();
    let file = dir.path().join("a.txt");
    write(&file, "a");
    let step = trash_step(&[&file]);
    fs::remove_file(dir.path().join(TEST_TRASH_DIR).join("a.txt")).expect("trash is emptied");

    let reversal = undo(step);

    assert_eq!(reversal.reversed, None);
    assert_eq!(
        reversal.problem.as_deref(),
        Some("“a.txt” is no longer in the Trash")
    );
    assert!(!file.exists());
}

#[test]
fn test_undo_of_a_move_moves_back_and_redo_moves_again() {
    let dir = temp_dir();
    let from = dir.path().join("from/a.txt");
    let to = dir.path().join("to/a.txt");
    write(&from, "a");
    fs::create_dir(dir.path().join("to")).expect("folder is created");
    fs::rename(&from, &to).expect("file is moved");
    let step = Step::Moved(vec![MovedItem {
        from: from.clone(),
        to: to.clone(),
    }]);

    let step = fully_reversed(undo(step));
    assert_eq!(read(&from), "a");
    assert!(!to.exists());

    fully_reversed(redo(step));
    assert_eq!(read(&to), "a");
    assert!(!from.exists());
}

#[test]
fn test_undo_of_a_move_never_replaces_what_took_the_old_place() {
    let dir = temp_dir();
    let from = dir.path().join("a.txt");
    let to = dir.path().join("b.txt");
    write(&to, "renamed");
    write(&from, "written since");
    let step = Step::Moved(vec![MovedItem {
        from: from.clone(),
        to: to.clone(),
    }]);

    let reversal = undo(step.clone());

    assert_eq!(reversal.reversed, None);
    assert_eq!(reversal.remaining, Some(step));
    assert!(reversal.problem.is_some());
    assert_eq!(read(&from), "written since");
    assert_eq!(read(&to), "renamed");
}

#[test]
fn test_undo_of_a_paste_trashes_the_copies_and_redo_copies_again() {
    let dir = temp_dir();
    let source = dir.path().join("from/pkg");
    let target = dir.path().join("to");
    write(&source.join("mod.rs"), "mod");
    write(&dir.path().join("from/a.txt"), "a");
    fs::create_dir(&target).expect("folder is created");
    let step = copy_step(&[source.clone(), dir.path().join("from/a.txt")], &target);

    let step = fully_reversed(undo(step));
    assert!(!target.join("pkg").exists());
    assert!(!target.join("a.txt").exists());
    // The copies are in the trash, not gone, and the originals are untouched.
    assert_eq!(read(&target.join(TEST_TRASH_DIR).join("pkg/mod.rs")), "mod");
    assert_eq!(read(&source.join("mod.rs")), "mod");

    let step = fully_reversed(redo(step));
    assert_eq!(read(&target.join("pkg/mod.rs")), "mod");
    assert_eq!(read(&target.join("a.txt")), "a");

    fully_reversed(undo(step));
    assert!(!target.join("pkg").exists());
}

#[test]
fn test_undo_of_a_paste_leaves_a_copy_that_was_changed() {
    let dir = temp_dir();
    let source = dir.path().join("from/pkg");
    let target = dir.path().join("to");
    write(&source.join("mod.rs"), "mod");
    fs::create_dir(&target).expect("folder is created");

    // A file added to a copied folder.
    let step = copy_step(std::slice::from_ref(&source), &target);
    write(&target.join("pkg/added.rs"), "added");
    let reversal = undo(step);
    assert_eq!(reversal.reversed, None);
    assert_eq!(
        reversal.problem.as_deref(),
        Some("“pkg” has changed since it was created, so it was left in place")
    );
    assert_eq!(read(&target.join("pkg/added.rs")), "added");

    // A copied file written to.
    let file = dir.path().join("from/a.txt");
    write(&file, "a");
    let step = copy_step(std::slice::from_ref(&file), &target);
    write(&target.join("a.txt"), "edited since");
    let reversal = undo(step);
    assert_eq!(reversal.reversed, None);
    assert!(reversal.problem.is_some());
    assert_eq!(read(&target.join("a.txt")), "edited since");
}

#[test]
fn test_undo_of_new_items_trashes_them_only_while_they_are_untouched() {
    let dir = temp_dir();
    let file = dir.path().join("new.txt");
    let folder = dir.path().join("new_folder");
    fs::File::create_new(&file).expect("file is created");
    fs::create_dir(&folder).expect("folder is created");
    let created = |path: &Path, origin: Origin| CreatedItem {
        path: path.to_path_buf(),
        origin,
        fingerprint: Fingerprint::of(path).expect("item is read"),
    };

    let new = |path: &Path, is_folder| Origin::New {
        path: path.to_path_buf(),
        is_folder,
    };
    let step = Step::Created(vec![
        created(&file, new(&file, false)),
        created(&folder, new(&folder, true)),
    ]);
    let step = fully_reversed(undo(step));
    assert!(!file.exists() && !folder.exists());

    // Redo makes them anew, and what is put in them afterwards keeps them from being undone.
    let step = fully_reversed(redo(step));
    assert_eq!(read(&file), "");
    assert!(folder.is_dir());
    write(&file, "typed since");
    write(&folder.join("inside.txt"), "x");

    let reversal = undo(step);
    assert_eq!(reversal.reversed, None);
    assert!(reversal.problem.is_some());
    assert_eq!(read(&file), "typed since");
    assert!(folder.join("inside.txt").exists());
}

#[test]
fn test_redo_never_replaces_what_appeared_in_the_meantime() {
    let dir = temp_dir();
    let source = dir.path().join("a.txt");
    let target = dir.path().join("to");
    write(&source, "a");
    fs::create_dir(&target).expect("folder is created");
    let step = fully_reversed(undo(copy_step(std::slice::from_ref(&source), &target)));
    write(&target.join("a.txt"), "someone else's");

    let reversal = redo(step);

    assert_eq!(reversal.reversed, None);
    assert!(reversal.problem.is_some());
    assert_eq!(read(&target.join("a.txt")), "someone else's");
}

#[test]
fn test_summaries_name_a_single_item_and_count_several() {
    let trashed = |names: &[&str]| {
        Step::Trashed(
            names
                .iter()
                .map(|name| TrashedItem {
                    original: PathBuf::from("/repo").join(name),
                    in_trash: PathBuf::from("/trash").join(name),
                })
                .collect(),
        )
    };
    assert_eq!(
        trashed(&["a.txt"]).summary(false),
        "Moved “a.txt” to the Trash"
    );
    assert_eq!(trashed(&["a.txt"]).summary(true), "Put “a.txt” back");
    assert_eq!(
        trashed(&["a.txt", "b.txt"]).summary(false),
        "Moved 2 items to the Trash"
    );

    let renamed = Step::Moved(vec![MovedItem {
        from: "/repo/old.txt".into(),
        to: "/repo/new.txt".into(),
    }]);
    assert_eq!(renamed.summary(false), "Renamed “old.txt” to “new.txt”");
    assert_eq!(renamed.summary(true), "Renamed “new.txt” back to “old.txt”");

    let moved = Step::Moved(vec![MovedItem {
        from: "/repo/a.txt".into(),
        to: "/repo/pkg/a.txt".into(),
    }]);
    assert_eq!(moved.summary(false), "Moved “a.txt”");
    assert_eq!(moved.summary(true), "Moved “a.txt” back");
}

#[test]
fn test_undo_of_a_nested_new_file_takes_the_new_folders_and_redo_makes_them_again() {
    let dir = temp_dir();
    let new_item = file_ops::create_new(dir.path(), "components/forms/Login.tsx", false)
        .expect("file is created");
    let step = Step::Created(vec![CreatedItem {
        path: new_item.created_root.clone(),
        origin: Origin::New {
            path: new_item.path.clone(),
            is_folder: false,
        },
        fingerprint: new_item.fingerprint,
    }]);
    assert_eq!(step.summary(false), "Created “Login.tsx”");

    let step = fully_reversed(undo(step));
    assert!(!dir.path().join("components").exists());

    let step = fully_reversed(redo(step));
    assert_eq!(read(&new_item.path), "");

    // Something else put into one of the new folders keeps them all in place.
    write(&dir.path().join("components/other.tsx"), "x");
    let reversal = undo(step);
    assert_eq!(reversal.reversed, None);
    assert!(new_item.path.exists());
}
