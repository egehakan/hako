use std::io::{Seek, Write as _};

use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::*;
use crate::protocol::ErrorCode;

fn tree() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap().to_owned();
    (directory, path)
}

/// An entry of a zip file made for a test.
enum Entry<'a> {
    Directory(&'a str),
    File(&'a str, &'a str),
    Link(&'a str, &'a str),
}

fn make_zip(path: &str, entries: &[Entry]) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default();
    for entry in entries {
        match entry {
            Entry::Directory(name) => writer.add_directory(*name, options).unwrap(),
            Entry::File(name, content) => {
                writer
                    .start_file(*name, options.unix_permissions(0o640))
                    .unwrap();
                writer.write_all(content.as_bytes()).unwrap();
            }
            Entry::Link(name, target) => writer.add_symlink(*name, *target, options).unwrap(),
        }
    }
    writer.finish().unwrap();
}

fn params(path: &str, destination: &str) -> FsExtractParams {
    FsExtractParams {
        path: path.to_owned(),
        destination: destination.to_owned(),
    }
}

fn run(root: &str, entries: &[Entry]) -> Result<FsExtract, RpcError> {
    let zip = format!("{root}/photos.zip");
    make_zip(&zip, entries);
    extract(&params(&zip, root), LIMITS, &Deadline::from_now())
}

fn listing(root: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn unpacks_the_contents_of_a_single_top_directory_into_one_named_after_it() {
    let (_directory, root) = tree();
    let entries = [
        Entry::Directory("album/"),
        Entry::File("album/a.txt", "a"),
        Entry::File("album/deep/b.txt", "b"),
        Entry::Link("album/latest", "a.txt"),
        Entry::Link("album/deep/up", "../a.txt"),
        Entry::File("__MACOSX/album/._a.txt", "resource fork"),
        Entry::File("album/.DS_Store", "finder"),
    ];
    let extracted = run(&root, &entries).unwrap();
    assert_eq!(extracted.path, format!("{root}/album"));
    assert_eq!(extracted.entries, 4);
    let album = format!("{root}/album");
    assert_eq!(
        fs::read_to_string(format!("{album}/deep/b.txt")).unwrap(),
        "b"
    );
    assert_eq!(fs::read_to_string(format!("{album}/latest")).unwrap(), "a");
    assert_eq!(
        fs::read_link(format!("{album}/deep/up")).unwrap(),
        Path::new("../a.txt")
    );
    assert_eq!(
        fs::metadata(format!("{album}/a.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(listing(&album), ["a.txt", "deep", "latest"]);

    let again = extract(
        &params(&format!("{root}/photos.zip"), &root),
        LIMITS,
        &Deadline::from_now(),
    )
    .unwrap();
    assert_eq!(again.path, format!("{root}/album (2)"));
    assert_eq!(listing(&again.path), ["a.txt", "deep", "latest"]);
}

#[test]
fn unpacks_anything_else_into_a_new_directory_named_after_the_file() {
    let shapes: [(&[Entry], &[&str]); 3] = [
        (
            &[Entry::File("album/a.txt", "a"), Entry::File("b.txt", "b")],
            &["album", "b.txt"],
        ),
        (
            &[
                Entry::File("album/a.txt", "a"),
                Entry::File("other/b.txt", "b"),
            ],
            &["album", "other"],
        ),
        (&[Entry::File("notes.txt", "n")], &["notes.txt"]),
    ];
    for (entries, expected) in shapes {
        let (_directory, root) = tree();
        let extracted = run(&root, entries).unwrap();
        assert_eq!(extracted.path, format!("{root}/photos"));
        assert_eq!(listing(&extracted.path), expected);
    }
}

#[test]
fn refuses_entries_that_would_land_outside_and_leaves_nothing_behind() {
    let hostile: [&[Entry]; 8] = [
        &[Entry::File("ok.txt", "ok"), Entry::File("../evil.txt", "x")],
        &[Entry::File("/etc/evil.txt", "x")],
        &[Entry::File("a/../../evil.txt", "x")],
        &[Entry::Link("link", "../outside")],
        &[Entry::Link("link", "/etc")],
        &[
            Entry::File("album/a.txt", "a"),
            Entry::Link("album/link", "../outside"),
        ],
        &[Entry::Link("here", "."), Entry::Link("link", "here/..")],
        &[
            Entry::Directory("sub/"),
            Entry::Link("link", "sub"),
            Entry::File("link/through.txt", "x"),
        ],
    ];
    for entries in hostile {
        let (_directory, root) = tree();
        let error = run(&root, entries).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest, "{}", error.message);
        assert_eq!(listing(&root), ["photos.zip"]);
    }
}

#[test]
fn refuses_an_encrypted_archive() {
    let (_directory, root) = tree();
    let zip = format!("{root}/secret.zip");
    let mut writer = ZipWriter::new(File::create(&zip).unwrap());
    writer
        .start_file(
            "a.txt",
            SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "password"),
        )
        .unwrap();
    writer.write_all(b"secret").unwrap();
    writer.finish().unwrap();
    let error = extract(&params(&zip, &root), LIMITS, &Deadline::from_now()).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert_eq!(listing(&root), ["secret.zip"]);
}

#[test]
fn refuses_an_archive_over_its_limits() {
    let (_directory, root) = tree();
    let zip = format!("{root}/photos.zip");
    make_zip(
        &zip,
        &[Entry::File("a.txt", "12345"), Entry::File("b.txt", "67890")],
    );
    let attempt =
        |limits| extract(&params(&zip, &root), limits, &Deadline::from_now()).unwrap_err();
    let error = attempt(Limits {
        bytes: 9,
        entries: 10,
    });
    assert_eq!((error.code, error.limit), (ErrorCode::TooLarge, Some(9)));
    let error = attempt(Limits {
        bytes: 100,
        entries: 1,
    });
    assert_eq!((error.code, error.limit), (ErrorCode::TooLarge, Some(1)));
    assert_eq!(listing(&root), ["photos.zip"]);
}

#[test]
fn reports_a_damaged_archive_and_removes_what_it_began() {
    let (_directory, root) = tree();
    let zip = format!("{root}/photos.zip");
    fs::write(&zip, b"this is not a zip file").unwrap();
    let error = extract(&params(&zip, &root), LIMITS, &Deadline::from_now()).unwrap_err();
    assert_eq!(error.code, ErrorCode::Failed);

    // A good directory with a file whose stored bytes are then spoiled.
    let numbers = (0..2000)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");
    make_zip(&zip, &[Entry::File("a.txt", &numbers)]);
    let mut file = OpenOptions::new().write(true).open(&zip).unwrap();
    file.seek(std::io::SeekFrom::Start(60)).unwrap();
    file.write_all(&[0xFF; 32]).unwrap();
    drop(file);
    let error = extract(&params(&zip, &root), LIMITS, &Deadline::from_now()).unwrap_err();
    assert_eq!(error.code, ErrorCode::Failed);
    assert_eq!(listing(&root), ["photos.zip"]);
}

#[test]
fn stops_at_the_deadline_and_removes_the_new_directory() {
    let (_directory, root) = tree();
    let zip = format!("{root}/photos.zip");
    let names: Vec<String> = (0..10).map(|n| format!("file{n}.txt")).collect();
    let entries: Vec<Entry> = names.iter().map(|name| Entry::File(name, "text")).collect();
    make_zip(&zip, &entries);
    let error = extract(&params(&zip, &root), LIMITS, &Deadline::after_checks(5)).unwrap_err();
    assert_eq!(error.message, "Stopped after 240 seconds");
    assert_eq!(listing(&root), ["photos.zip"]);
}

#[test]
fn refuses_a_destination_that_is_not_a_directory() {
    let (_directory, root) = tree();
    let zip = format!("{root}/photos.zip");
    make_zip(&zip, &[Entry::File("a.txt", "a")]);
    let code = |path: &str, destination: &str| {
        extract(&params(path, destination), LIMITS, &Deadline::from_now())
            .unwrap_err()
            .code
    };
    assert_eq!(code(&zip, &zip), ErrorCode::NotADirectory);
    assert_eq!(code(&zip, &format!("{root}/missing")), ErrorCode::NotFound);
    assert_eq!(
        code(&format!("{root}/missing.zip"), &root),
        ErrorCode::NotFound
    );
    assert_eq!(code(&root, &root), ErrorCode::IsADirectory);
}
