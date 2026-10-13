use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn journal_file(directory: &tempfile::TempDir) -> Option<PathBuf> {
    Some(directory.path().join("state").join(JOURNAL_FILE_NAME))
}

#[test]
fn a_load_that_finishes_leaves_nothing_behind() {
    let directory = tempfile::tempdir().unwrap();
    let file = Path::new("/repo/src/main.rs");

    let mut journal = LoadJournal::open(journal_file(&directory));
    journal.begin(file);
    journal.finish(file);
    drop(journal);

    let next_run = LoadJournal::open(journal_file(&directory));
    assert!(!next_run.was_interrupted(file));
}

#[test]
fn a_load_the_app_did_not_survive_is_interrupted_in_the_next_run() {
    let directory = tempfile::tempdir().unwrap();
    let dump = Path::new("/repo/dump.sql");
    let other = Path::new("/repo/notes.txt");

    let mut journal = LoadJournal::open(journal_file(&directory));
    journal.begin(other);
    journal.begin(dump);
    journal.finish(other);
    // The process ends here without `finish`: the entry is already on disk.
    drop(journal);

    let next_run = LoadJournal::open(journal_file(&directory));
    assert!(next_run.was_interrupted(dump));
    assert!(!next_run.was_interrupted(other));
}

#[test]
fn an_interruption_is_remembered_until_the_user_deals_with_it() {
    let directory = tempfile::tempdir().unwrap();
    let dump = Path::new("/repo/dump.sql");

    let mut journal = LoadJournal::open(journal_file(&directory));
    journal.begin(dump);
    drop(journal);

    // A run that stops for another reason, with the placeholder untouched, changes nothing.
    drop(LoadJournal::open(journal_file(&directory)));
    let mut third_run = LoadJournal::open(journal_file(&directory));
    assert!(third_run.was_interrupted(dump));

    third_run.forget_interruption(dump);
    assert!(!third_run.was_interrupted(dump));
    drop(third_run);
    assert!(!LoadJournal::open(journal_file(&directory)).was_interrupted(dump));
}

#[test]
fn a_file_that_takes_the_app_down_again_is_interrupted_again() {
    let directory = tempfile::tempdir().unwrap();
    let dump = Path::new("/repo/dump.sql");

    let mut journal = LoadJournal::open(journal_file(&directory));
    journal.begin(dump);
    drop(journal);

    // The user asks for the file after all, and the load fails the same way.
    let mut second_run = LoadJournal::open(journal_file(&directory));
    second_run.forget_interruption(dump);
    second_run.begin(dump);
    drop(second_run);

    assert!(LoadJournal::open(journal_file(&directory)).was_interrupted(dump));
}

#[test]
fn an_orderly_stop_interrupts_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let file = Path::new("/repo/large.log");

    let mut journal = LoadJournal::open(journal_file(&directory));
    journal.begin(file);
    journal.finish_all();
    drop(journal);

    assert!(!LoadJournal::open(journal_file(&directory)).was_interrupted(file));
}

#[test]
fn a_journal_that_cannot_be_read_is_an_empty_one() {
    let directory = tempfile::tempdir().unwrap();
    let file = journal_file(&directory).unwrap();
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"{ not json").unwrap();

    let mut journal = LoadJournal::open(Some(file.clone()));
    assert!(!journal.was_interrupted(Path::new("/repo/dump.sql")));

    // It is replaced by a readable one at the next entry.
    journal.begin(Path::new("/repo/dump.sql"));
    drop(journal);
    assert!(LoadJournal::open(Some(file)).was_interrupted(Path::new("/repo/dump.sql")));
}

#[test]
fn a_journal_without_a_file_lasts_as_long_as_the_process() {
    let mut journal = LoadJournal::open(None);
    journal.begin(Path::new("/repo/a.rs"));
    journal.finish(Path::new("/repo/a.rs"));
    assert!(!journal.was_interrupted(Path::new("/repo/a.rs")));
}
