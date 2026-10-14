use std::io::Read as _;
use std::os::unix::fs::symlink;

use tempfile::TempDir;
use zip::ZipArchive;

use super::*;
use crate::protocol::ErrorCode;

const LIMIT: u64 = 200 * 1024 * 1024;

fn tree() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap().to_owned();
    (directory, path)
}

fn write(path: &str, content: &[u8]) {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// A project with each kind of directory that is left out, and one called `target` that is not
/// a build's output.
fn project(root: &str) -> String {
    let project = format!("{root}/project");
    write(&format!("{project}/src/main.rs"), b"fn main() {}");
    write(&format!("{project}/.git/HEAD"), b"ref");
    write(&format!("{project}/node_modules/x/index.js"), b"x");
    write(&format!("{project}/target/CACHEDIR.TAG"), b"tag");
    write(&format!("{project}/web/target/.rustc_info.json"), b"{}");
    write(&format!("{project}/docs/target/plan.md"), b"plan");
    symlink("src/main.rs", format!("{project}/main")).unwrap();
    project
}

fn params(path: &str, skip_heavy: Option<bool>) -> FsArchiveParams {
    FsArchiveParams {
        path: path.to_owned(),
        skip_heavy,
    }
}

fn names(archive: &str) -> Vec<String> {
    let archive = ZipArchive::new(File::open(archive).unwrap()).unwrap();
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    names
}

fn area_is_empty(area: &Path) -> bool {
    fs::read_dir(area).map_or(true, |mut entries| entries.next().is_none())
}

#[test]
fn zips_a_directory_without_what_can_be_fetched_or_built_again() {
    let (_directory, root) = tree();
    let project = project(&root);
    let area = Path::new(&root).join("archives");

    let archive = create(&params(&project, None), &area, LIMIT, &Deadline::from_now()).unwrap();
    assert_eq!(archive.name, "project.zip");
    assert_eq!(archive.skipped, 4);
    assert_eq!(archive.size, fs::metadata(&archive.path).unwrap().len());
    assert!(archive.path.starts_with(area.to_str().unwrap()));
    assert_eq!(
        names(&archive.path),
        [
            "project/",
            "project/docs/",
            "project/docs/target/",
            "project/docs/target/plan.md",
            "project/main",
            "project/src/",
            "project/src/main.rs",
            "project/web/",
        ]
    );

    let mut zip = ZipArchive::new(File::open(&archive.path).unwrap()).unwrap();
    let mut link = zip.by_name("project/main").unwrap();
    assert!(link.is_symlink());
    let mut target = String::new();
    link.read_to_string(&mut target).unwrap();
    assert_eq!(target, "src/main.rs");
    drop(link);
    let mut file = zip.by_name("project/src/main.rs").unwrap();
    assert_eq!(file.compression(), CompressionMethod::Deflated);
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    assert_eq!(content, "fn main() {}");
}

#[test]
fn zips_everything_when_asked_to() {
    let (_directory, root) = tree();
    let project = project(&root);
    let area = Path::new(&root).join("archives");
    let archive = create(
        &params(&project, Some(false)),
        &area,
        LIMIT,
        &Deadline::from_now(),
    )
    .unwrap();
    assert_eq!(archive.skipped, 0);
    let names = names(&archive.path);
    for name in [
        "project/.git/HEAD",
        "project/node_modules/x/index.js",
        "project/target/CACHEDIR.TAG",
    ] {
        assert!(names.iter().any(|listed| listed == name), "{name}");
    }
}

#[test]
fn leaves_out_what_cannot_be_read() {
    let (_directory, root) = tree();
    let folder = format!("{root}/folder");
    write(&format!("{folder}/open.txt"), b"open");
    write(&format!("{folder}/secret.txt"), b"secret");
    fs::set_permissions(
        format!("{folder}/secret.txt"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let area = Path::new(&root).join("archives");
    let archive = create(&params(&folder, None), &area, LIMIT, &Deadline::from_now()).unwrap();
    assert_eq!(names(&archive.path), ["folder/", "folder/open.txt"]);
}

#[test]
fn refuses_a_file_and_what_is_not_there() {
    let (_directory, root) = tree();
    write(&format!("{root}/file.txt"), b"f");
    let area = Path::new(&root).join("archives");
    let code = |path: &str| {
        create(&params(path, None), &area, LIMIT, &Deadline::from_now())
            .unwrap_err()
            .code
    };
    assert_eq!(code(&format!("{root}/file.txt")), ErrorCode::BadRequest);
    assert_eq!(code(&format!("{root}/missing")), ErrorCode::NotFound);
    assert_eq!(code("relative"), ErrorCode::BadRequest);
}

#[test]
fn stops_at_the_limit_and_at_the_deadline_and_leaves_nothing_behind() {
    let (_directory, root) = tree();
    let folder = format!("{root}/folder");
    // Bytes that do not compress.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let noise: Vec<u8> = (0..3 * COPY_BUFFER_BYTES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect();
    write(&format!("{folder}/noise.bin"), &noise);
    for n in 0..10 {
        write(&format!("{folder}/file{n}.txt"), b"text");
    }
    let area = Path::new(&root).join("archives");

    let error = create(
        &params(&folder, None),
        &area,
        1024 * 1024,
        &Deadline::from_now(),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    assert_eq!(error.limit, Some(1024 * 1024));
    assert!(area_is_empty(&area));

    let error = create(
        &params(&folder, None),
        &area,
        LIMIT,
        &Deadline::after_checks(3),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Failed);
    assert_eq!(error.message, "Stopped after 240 seconds");
    assert!(area_is_empty(&area));
}

#[test]
fn removes_the_archives_that_have_been_kept_long_enough_and_nothing_else() {
    let (_directory, root) = tree();
    let area = Path::new(&root).join("archives");
    let folder = format!("{root}/folder");
    write(&format!("{folder}/a.txt"), b"a");
    let archive = create(&params(&folder, None), &area, LIMIT, &Deadline::from_now()).unwrap();
    write(area.join("unknown/keep.txt").to_str().unwrap(), b"k");

    clean(&area, SystemTime::now(), KEPT_FOR);
    assert!(Path::new(&archive.path).exists());
    clean(&area, SystemTime::now() + KEPT_FOR, KEPT_FOR);
    assert!(!Path::new(&archive.path).exists());
    let mut left: Vec<String> = fs::read_dir(&area)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["unknown"]);
}
