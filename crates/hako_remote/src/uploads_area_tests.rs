use std::fs::{File, Permissions};
use std::os::unix::fs::{PermissionsExt as _, symlink};

use super::*;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn file_changed_at(path: &Path, modified: SystemTime) {
    File::create(path).unwrap().set_modified(modified).unwrap();
}

#[test]
fn names_a_day_as_year_month_and_day() {
    let today = today();
    assert!(is_day_name(&today), "{today}");
    assert!(!is_day_name("2026-1-13"));
    assert!(!is_day_name("2026_10_13"));
    assert!(!is_day_name("notes"));
}

#[test]
fn creates_the_day_directory_for_the_user_only() {
    let home = tempfile::tempdir().unwrap();
    let area = home.path().join(".hako/uploads");

    let directory = day_directory(&area, "2026-10-13").unwrap();

    assert_eq!(directory, area.join("2026-10-13"));
    for created in [&area, &directory] {
        let mode = fs::metadata(created).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", created.display());
    }
    // A directory that is already there is fine.
    day_directory(&area, "2026-10-13").unwrap();
}

#[test]
fn deletes_only_files_older_than_seven_days_in_day_directories() {
    let area = tempfile::tempdir().unwrap();
    let area = area.path();
    let now = SystemTime::now();
    let old_day = day_directory(area, "2026-10-01").unwrap();
    let mixed_day = day_directory(area, "2026-10-08").unwrap();
    let today = day_directory(area, "2026-10-13").unwrap();
    let other = area.join("keep");
    fs::create_dir(&other).unwrap();

    file_changed_at(&old_day.join("a.png"), now - KEPT_FOR - DAY);
    file_changed_at(&mixed_day.join("old.png"), now - KEPT_FOR - DAY);
    file_changed_at(&mixed_day.join("new.png"), now - KEPT_FOR + DAY);
    file_changed_at(&other.join("old.png"), now - KEPT_FOR - DAY);
    file_changed_at(&area.join("old.png"), now - KEPT_FOR - DAY);
    let nested = mixed_day.join("nested");
    fs::create_dir(&nested).unwrap();
    file_changed_at(&nested.join("old.png"), now - KEPT_FOR - DAY);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("target.png");
    file_changed_at(&target, now - KEPT_FOR - DAY);
    symlink(&target, mixed_day.join("link.png")).unwrap();

    clean(area, now, "2026-10-13");

    assert!(!old_day.exists());
    assert!(!mixed_day.join("old.png").exists());
    assert!(mixed_day.join("new.png").exists());
    assert!(nested.join("old.png").exists());
    assert!(mixed_day.join("link.png").exists());
    assert!(target.exists());
    assert!(other.join("old.png").exists());
    assert!(area.join("old.png").exists());
    assert!(today.exists(), "today's directory stays even when empty");
}

#[test]
fn leaves_what_it_may_not_delete() {
    let area = tempfile::tempdir().unwrap();
    let area = area.path();
    let now = SystemTime::now();
    let day = day_directory(area, "2026-10-01").unwrap();
    file_changed_at(&day.join("a.png"), now - KEPT_FOR - DAY);
    fs::set_permissions(&day, Permissions::from_mode(0o500)).unwrap();

    clean(area, now, "2026-10-13");

    fs::set_permissions(&day, Permissions::from_mode(0o700)).unwrap();
    assert!(day.join("a.png").exists());
}

#[test]
fn does_nothing_without_an_area() {
    let home = tempfile::tempdir().unwrap();
    clean(
        &home.path().join("missing"),
        SystemTime::now(),
        "2026-10-13",
    );
}
