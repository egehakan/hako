use std::fmt::Debug;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::os::unix::net::UnixListener;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::protocol::{Folder, Pane, PaneKind, Tab, Window};

/// A temporary directory and its path as the protocol writes paths.
fn tree() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap().to_owned();
    (directory, path)
}

fn code<T: Debug>(result: Result<T, RpcError>) -> ErrorCode {
    result.unwrap_err().code
}

fn names(list: &FsList) -> Vec<&str> {
    list.entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect()
}

fn entry<'a>(list: &'a FsList, name: &str) -> &'a FsEntry {
    list.entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap()
}

fn set_mode(path: &str, mode: u32) {
    fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

fn write_params(path: &str, content: &str, expected_mtime: Option<i64>) -> FsWriteParams {
    FsWriteParams {
        path: path.to_owned(),
        content: content.to_owned(),
        expected_mtime,
    }
}

fn pane(cwd: Option<&str>) -> Pane {
    Pane {
        id: "1".to_owned(),
        kind: PaneKind::Terminal,
        title: String::new(),
        focused: false,
        session_id: None,
        cwd: cwd.map(str::to_owned),
        agent: None,
    }
}

#[test]
fn starts_from_the_folders_then_home_then_the_terminals_each_path_once() {
    let folder = |name: &str, directory: &str| Folder {
        id: name.to_owned(),
        name: name.to_owned(),
        directory: directory.to_owned(),
        color: None,
        icon: None,
    };
    let window = |folders, panes| Window {
        id: "1".to_owned(),
        active: false,
        folders,
        tabs: vec![Tab {
            id: "1".to_owned(),
            title: String::new(),
            active: false,
            folder_id: None,
            panes,
        }],
    };
    let workspace = Workspace {
        windows: vec![
            window(
                vec![folder("hako", "/src/hako")],
                vec![pane(Some("/src/hako")), pane(Some("/")), pane(None)],
            ),
            window(
                vec![folder("Home", "/Users/ege"), folder("again", "/src/hako")],
                vec![pane(Some("/Users/ege")), pane(Some("/src/hako/app"))],
            ),
        ],
    };

    let roots = roots("/Users/ege", &workspace);

    assert_eq!(roots.home, "/Users/ege");
    let listed: Vec<_> = roots
        .roots
        .iter()
        .map(|root| (root.path.as_str(), root.name.as_str(), root.source))
        .collect();
    assert_eq!(
        listed,
        [
            ("/src/hako", "hako", FsRootSource::Folder),
            ("/Users/ege", "Home", FsRootSource::Folder),
            ("/", "/", FsRootSource::Terminal),
            ("/src/hako/app", "app", FsRootSource::Terminal),
        ]
    );
}

#[test]
fn lists_the_home_directory_when_no_folder_stands_for_it() {
    let roots = roots("/Users/ege", &Workspace::default());

    assert_eq!(
        roots.roots,
        [FsRoot {
            path: "/Users/ege".to_owned(),
            name: "Home".to_owned(),
            source: FsRootSource::Home,
        }]
    );
}

#[test]
fn accepts_only_absolute_paths_in_their_normal_form() {
    for path in [
        "", "~", "~/x", "a/b", "/a//b", "/a/", "/a/../b", "/a/./b", "//", "/a\0b",
    ] {
        assert_eq!(code(list(path)), ErrorCode::BadRequest, "{path:?}");
        assert_eq!(code(read(path)), ErrorCode::BadRequest, "{path:?}");
        assert_eq!(
            code(write(&write_params(path, "", None))),
            ErrorCode::BadRequest,
            "{path:?}"
        );
    }
    assert!(checked("/").is_ok());
    assert!(checked("/a/.b/c..d").is_ok());
}

#[test]
fn lists_directories_first_and_then_by_name_whatever_the_case() {
    let (_directory, root) = tree();
    for name in ["b.txt", "A.txt", ".hidden", "c.txt"] {
        fs::write(format!("{root}/{name}"), "").unwrap();
    }
    for name in ["Zeta", "alpha"] {
        fs::create_dir(format!("{root}/{name}")).unwrap();
    }
    symlink("Zeta", format!("{root}/link")).unwrap();

    let list = list(&root).unwrap();

    assert_eq!(
        names(&list),
        [
            "alpha", "link", "Zeta", ".hidden", "A.txt", "b.txt", "c.txt"
        ]
    );
    assert!(!list.truncated);
}

#[test]
fn orders_names_that_differ_only_in_case_by_the_names_themselves() {
    let file = |name: &str| FsEntry {
        name: name.to_owned(),
        kind: FsEntryKind::File,
        target: None,
        size: 0,
        mtime: 0,
    };
    let mut entries = vec![file("readme"), file("b"), file("README"), file("Readme")];

    sort(&mut entries);

    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["b", "README", "Readme", "readme"]);
}

#[test]
fn describes_each_kind_of_entry() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/file"), "12345").unwrap();
    fs::create_dir(format!("{root}/dir")).unwrap();
    fs::write(format!("{root}/dir/inside"), "1").unwrap();
    symlink("file", format!("{root}/to-file")).unwrap();
    symlink("dir", format!("{root}/to-dir")).unwrap();
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();
    let _socket = UnixListener::bind(format!("{root}/socket")).unwrap();

    let list = list(&root).unwrap();

    let file = entry(&list, "file");
    assert_eq!(
        (file.kind, file.target, file.size),
        (FsEntryKind::File, None, 5)
    );
    assert!(file.mtime > 1_600_000_000_000);
    let dir = entry(&list, "dir");
    assert_eq!(
        (dir.kind, dir.target, dir.size),
        (FsEntryKind::Dir, None, 0)
    );
    let to_file = entry(&list, "to-file");
    assert_eq!(
        (to_file.kind, to_file.target, to_file.size, to_file.mtime),
        (
            FsEntryKind::Symlink,
            Some(FsLinkTarget::File),
            5,
            file.mtime
        )
    );
    let to_dir = entry(&list, "to-dir");
    assert_eq!(
        (to_dir.kind, to_dir.target, to_dir.size),
        (FsEntryKind::Symlink, Some(FsLinkTarget::Dir), 0)
    );
    let to_nowhere = entry(&list, "to-nowhere");
    assert_eq!(
        (to_nowhere.kind, to_nowhere.target, to_nowhere.size),
        (FsEntryKind::Symlink, None, 0)
    );
    assert!(to_nowhere.mtime > 1_600_000_000_000);
    assert_eq!(entry(&list, "socket").kind, FsEntryKind::Other);
}

#[test]
fn lists_the_directory_a_link_points_to() {
    let (_directory, root) = tree();
    fs::create_dir(format!("{root}/dir")).unwrap();
    fs::write(format!("{root}/dir/inside"), "1").unwrap();
    symlink("dir", format!("{root}/link")).unwrap();

    assert_eq!(names(&list(&format!("{root}/link")).unwrap()), ["inside"]);
    assert_eq!(
        read(&format!("{root}/link/inside")).unwrap(),
        read(&format!("{root}/dir/inside")).unwrap()
    );
}

#[test]
fn says_when_a_directory_holds_more_than_it_lists() {
    let (_directory, root) = tree();
    for name in ["e", "d", "c", "b", "a"] {
        fs::write(format!("{root}/{name}"), "").unwrap();
    }

    let list = list_at_most(&root, 3).unwrap();

    assert_eq!(names(&list), ["a", "b", "c"]);
    assert!(list.truncated);
    assert!(!list_at_most(&root, 5).unwrap().truncated);
}

#[test]
fn refuses_to_list_what_is_not_a_directory_it_may_read() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/file"), "").unwrap();
    fs::create_dir(format!("{root}/closed")).unwrap();
    set_mode(&format!("{root}/closed"), 0o000);

    assert_eq!(code(list(&format!("{root}/missing"))), ErrorCode::NotFound);
    assert_eq!(
        code(list(&format!("{root}/file/below"))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(list(&format!("{root}/file"))),
        ErrorCode::NotADirectory
    );
    let refused = list(&format!("{root}/closed")).unwrap_err();
    assert_eq!(refused.code, ErrorCode::PermissionDenied);
    assert_eq!(refused.message, format!("Permission denied: {root}/closed"));

    set_mode(&format!("{root}/closed"), 0o755);
}

#[test]
fn takes_only_a_directory_it_may_read_for_a_readable_directory() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/file"), "").unwrap();
    fs::create_dir(format!("{root}/open")).unwrap();
    symlink("open", format!("{root}/to-open")).unwrap();
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();
    fs::create_dir(format!("{root}/closed")).unwrap();
    set_mode(&format!("{root}/closed"), 0o000);

    assert_eq!(readable_directory(&format!("{root}/open")), Ok(()));
    assert_eq!(readable_directory(&format!("{root}/to-open")), Ok(()));
    assert_eq!(
        code(readable_directory(&format!("{root}/open/"))),
        ErrorCode::BadRequest
    );
    assert_eq!(code(readable_directory("open")), ErrorCode::BadRequest);
    assert_eq!(
        code(readable_directory(&format!("{root}/missing"))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(readable_directory(&format!("{root}/to-nowhere"))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(readable_directory(&format!("{root}/file"))),
        ErrorCode::NotADirectory
    );
    assert_eq!(
        code(readable_directory(&format!("{root}/closed"))),
        ErrorCode::PermissionDenied
    );

    set_mode(&format!("{root}/closed"), 0o755);
}

#[test]
fn reads_text_exactly_as_it_is() {
    let (_directory, root) = tree();
    let path = format!("{root}/notes.txt");
    let content = "\u{feff}one\r\ntwo\nthree \u{1f4e6}";
    fs::write(&path, content).unwrap();

    let listed = list(&root).unwrap();

    assert_eq!(
        read(&path).unwrap(),
        FsRead::Text {
            content: content.to_owned(),
            mtime: entry(&listed, "notes.txt").mtime,
            size: content.len() as u64,
        }
    );
}

#[test]
fn reads_an_empty_file_as_text() {
    let (_directory, root) = tree();
    let path = format!("{root}/empty");
    fs::write(&path, "").unwrap();

    let FsRead::Text { content, size, .. } = read(&path).unwrap() else {
        panic!("an empty file is text");
    };
    assert_eq!((content.as_str(), size), ("", 0));
}

#[test]
fn takes_a_file_with_a_nul_byte_or_invalid_utf8_for_binary() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/nul"), b"text\0more").unwrap();
    fs::write(format!("{root}/latin1.txt"), [0x63, 0x61, 0x66, 0xe9]).unwrap();

    assert_eq!(
        read(&format!("{root}/nul")).unwrap(),
        FsRead::Binary { size: 9 }
    );
    assert_eq!(
        read(&format!("{root}/latin1.txt")).unwrap(),
        FsRead::Binary { size: 4 }
    );
}

#[test]
fn sends_an_image_for_its_extension_alone() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/Photo.JPG"), [1, 2, 3]).unwrap();
    fs::write(format!("{root}/drawing.svg"), "<svg/>").unwrap();

    let FsRead::Image {
        mime, data, size, ..
    } = read(&format!("{root}/Photo.JPG")).unwrap()
    else {
        panic!("a .JPG file is an image");
    };
    assert_eq!(
        (mime.as_str(), data.as_str(), size),
        ("image/jpeg", "AQID", 3)
    );
    let FsRead::Image { mime, data, .. } = read(&format!("{root}/drawing.svg")).unwrap() else {
        panic!("a .svg file is an image");
    };
    assert_eq!(
        (mime.as_str(), data.as_str()),
        ("image/svg+xml", "PHN2Zy8+")
    );
    for (name, mime) in [
        ("a.png", Some("image/png")),
        ("a.jpeg", Some("image/jpeg")),
        ("a.GIF", Some("image/gif")),
        ("a.webp", Some("image/webp")),
        (".png", Some("image/png")),
        ("png", None),
        ("a.png.txt", None),
        ("dir.png/file", None),
    ] {
        assert_eq!(image_mime(&format!("/x/{name}")), mime, "{name}");
    }
}

#[test]
fn reports_a_file_over_its_limit_without_its_contents() {
    let (_directory, root) = tree();
    let long = |name: &str, len: u64| {
        let path = format!("{root}/{name}");
        File::create(&path).unwrap().set_len(len).unwrap();
        path
    };

    assert_eq!(
        read(&long("log.txt", TEXT_LIMIT + 1)).unwrap(),
        FsRead::TooLarge {
            size: 2_097_153,
            limit: 2_097_152,
        }
    );
    assert_eq!(
        read(&long("photo.png", IMAGE_LIMIT + 1)).unwrap(),
        FsRead::TooLarge {
            size: 5_242_881,
            limit: 5_242_880,
        }
    );
    assert_eq!(
        read(&long("zeros", TEXT_LIMIT)).unwrap(),
        FsRead::Binary { size: 2_097_152 }
    );
}

#[test]
fn reports_text_that_would_not_fit_in_a_frame_as_too_large() {
    let (_directory, root) = tree();
    let path = format!("{root}/control.txt");
    fs::write(&path, vec![0x01; TEXT_LIMIT as usize]).unwrap();

    assert_eq!(
        read(&path).unwrap(),
        FsRead::TooLarge {
            size: 2_097_152,
            limit: 2_097_152,
        }
    );
    assert_eq!(
        json_len("a\"\\\n\t\u{1}\u{7f}é"),
        2 + 1 + 2 + 2 + 2 + 2 + 6 + 1 + 2
    );
}

#[test]
fn refuses_to_read_what_is_not_a_file() {
    let (_directory, root) = tree();
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();
    symlink("loop", format!("{root}/loop")).unwrap();
    let _socket = UnixListener::bind(format!("{root}/socket")).unwrap();
    fs::write(format!("{root}/secret"), "x").unwrap();
    set_mode(&format!("{root}/secret"), 0o000);

    assert_eq!(code(read(&root)), ErrorCode::IsADirectory);
    assert_eq!(code(read(&format!("{root}/missing"))), ErrorCode::NotFound);
    assert_eq!(
        code(read(&format!("{root}/to-nowhere"))),
        ErrorCode::NotFound
    );
    assert_eq!(code(read(&format!("{root}/loop"))), ErrorCode::NotFound);
    assert_eq!(code(read(&format!("{root}/socket"))), ErrorCode::Failed);
    assert_eq!(
        code(read(&format!("{root}/secret"))),
        ErrorCode::PermissionDenied
    );
}

#[test]
fn replaces_the_contents_and_keeps_the_permissions() {
    let (_directory, root) = tree();
    let path = format!("{root}/script.sh");
    fs::write(&path, "old\n").unwrap();
    set_mode(&path, 0o750);
    let FsRead::Text { mtime, .. } = read(&path).unwrap() else {
        panic!("the file is text");
    };

    let written = write(&write_params(
        &path,
        "new text, no final newline",
        Some(mtime),
    ))
    .unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "new text, no final newline"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o750
    );
    let listed = list(&root).unwrap();
    assert_eq!(names(&listed), ["script.sh"]);
    assert_eq!(
        written,
        FsWrite {
            mtime: entry(&listed, "script.sh").mtime,
            size: 26,
        }
    );
    // The answer is what the next write has to expect.
    write(&write_params(&path, "again", Some(written.mtime))).unwrap();
}

#[test]
fn refuses_a_write_when_the_file_has_changed_since_it_was_read() {
    let (_directory, root) = tree();
    let path = format!("{root}/notes.txt");
    fs::write(&path, "theirs").unwrap();
    let read_at = mtime(&fs::metadata(&path).unwrap());
    let set_modified = |offset: i64| {
        let modified = UNIX_EPOCH + Duration::from_millis((read_at + offset) as u64);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    };

    for offset in [1, -1, 60_000, -60_000] {
        set_modified(offset);
        let refused = write(&write_params(&path, "mine", Some(read_at)));
        assert_eq!(code(refused), ErrorCode::Conflict, "{offset}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
    }

    write(&write_params(&path, "mine", None)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
}

#[test]
fn never_creates_a_file() {
    let (_directory, root) = tree();
    let missing = format!("{root}/missing.txt");
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();

    assert_eq!(
        code(write(&write_params(&missing, "x", None))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(write(&write_params(
            &format!("{root}/to-nowhere"),
            "x",
            None
        ))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(write(&write_params(&root, "x", None))),
        ErrorCode::IsADirectory
    );
    assert_eq!(names(&list(&root).unwrap()), ["to-nowhere"]);
}

#[test]
fn refuses_to_write_where_the_user_may_not() {
    let (_directory, root) = tree();
    let read_only = format!("{root}/read-only.txt");
    fs::write(&read_only, "kept").unwrap();
    set_mode(&read_only, 0o444);
    fs::create_dir(format!("{root}/closed")).unwrap();
    let inside = format!("{root}/closed/file.txt");
    fs::write(&inside, "kept").unwrap();
    set_mode(&format!("{root}/closed"), 0o555);

    let refused = write(&write_params(&read_only, "changed", None)).unwrap_err();
    assert_eq!(refused.code, ErrorCode::PermissionDenied);
    assert_eq!(refused.message, format!("Permission denied: {read_only}"));
    assert_eq!(
        code(write(&write_params(&inside, "changed", None))),
        ErrorCode::PermissionDenied
    );

    assert_eq!(fs::read_to_string(&read_only).unwrap(), "kept");
    assert_eq!(fs::read_to_string(&inside).unwrap(), "kept");
    assert_eq!(
        names(&list(&format!("{root}/closed")).unwrap()),
        ["file.txt"]
    );
    set_mode(&format!("{root}/closed"), 0o755);
}

#[test]
fn writes_through_a_link_to_the_file_it_points_to() {
    let (_directory, root) = tree();
    fs::create_dir(format!("{root}/real")).unwrap();
    fs::write(format!("{root}/real/file.txt"), "old").unwrap();
    symlink("real/file.txt", format!("{root}/link.txt")).unwrap();

    write(&write_params(&format!("{root}/link.txt"), "new", None)).unwrap();

    assert_eq!(
        fs::read_to_string(format!("{root}/real/file.txt")).unwrap(),
        "new"
    );
    assert!(
        fs::symlink_metadata(format!("{root}/link.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(names(&list(&format!("{root}/real")).unwrap()), ["file.txt"]);
}

#[test]
fn refuses_text_over_the_limit() {
    let (_directory, root) = tree();
    let path = format!("{root}/notes.txt");
    fs::write(&path, "kept").unwrap();
    let at_the_limit = "a".repeat(TEXT_LIMIT as usize);

    let too_long = format!("{at_the_limit}a");
    assert_eq!(
        code(write(&write_params(&path, &too_long, None))),
        ErrorCode::TooLarge
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "kept");

    assert_eq!(
        write(&write_params(&path, &at_the_limit, None))
            .unwrap()
            .size,
        TEXT_LIMIT
    );
}

#[test]
fn makes_one_directory() {
    let (_directory, root) = tree();
    fs::create_dir(format!("{root}/real")).unwrap();
    symlink("real", format!("{root}/link")).unwrap();
    let longest = "n".repeat(255);

    assert_eq!(mkdir(&format!("{root}/new")), Ok(()));
    assert_eq!(mkdir(&format!("{root}/new/below")), Ok(()));
    assert_eq!(mkdir(&format!("{root}/link/through a link")), Ok(()));
    assert_eq!(mkdir(&format!("{root}/.hidden")), Ok(()));
    assert_eq!(mkdir(&format!("{root}/{longest}")), Ok(()));

    assert!(Path::new(&format!("{root}/new/below")).is_dir());
    assert!(Path::new(&format!("{root}/real/through a link")).is_dir());
    assert!(Path::new(&format!("{root}/.hidden")).is_dir());
    assert!(Path::new(&format!("{root}/{longest}")).is_dir());
}

#[test]
fn never_makes_the_directories_on_the_way() {
    let (_directory, root) = tree();
    fs::write(format!("{root}/file"), "").unwrap();
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();

    let missing = mkdir(&format!("{root}/a/b")).unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
    assert_eq!(
        missing.message,
        format!("No such file or directory: {root}/a")
    );
    assert_eq!(
        code(mkdir(&format!("{root}/to-nowhere/b"))),
        ErrorCode::NotFound
    );
    let in_a_file = mkdir(&format!("{root}/file/b")).unwrap_err();
    assert_eq!(in_a_file.code, ErrorCode::NotADirectory);
    assert_eq!(in_a_file.message, format!("Not a directory: {root}/file"));
    assert_eq!(
        code(mkdir(&format!("{root}/file/b/c"))),
        ErrorCode::NotFound
    );

    assert_eq!(names(&list(&root).unwrap()), ["file", "to-nowhere"]);
}

#[test]
fn refuses_to_make_a_directory_where_something_is() {
    let (_directory, root) = tree();
    fs::create_dir(format!("{root}/directory")).unwrap();
    fs::write(format!("{root}/directory/kept"), "").unwrap();
    fs::write(format!("{root}/file"), "kept").unwrap();
    symlink("nowhere", format!("{root}/to-nowhere")).unwrap();

    let taken = mkdir(&format!("{root}/directory")).unwrap_err();
    assert_eq!(taken.code, ErrorCode::Conflict);
    assert_eq!(taken.message, format!("Already exists: {root}/directory"));
    assert_eq!(code(mkdir(&format!("{root}/file"))), ErrorCode::Conflict);
    assert_eq!(
        code(mkdir(&format!("{root}/to-nowhere"))),
        ErrorCode::Conflict
    );
    assert_eq!(code(mkdir("/")), ErrorCode::Conflict);

    assert_eq!(
        names(&list(&format!("{root}/directory")).unwrap()),
        ["kept"]
    );
    assert_eq!(fs::read_to_string(format!("{root}/file")).unwrap(), "kept");
}

#[test]
fn refuses_to_make_a_directory_where_the_user_may_not() {
    let (_directory, root) = tree();
    fs::create_dir(format!("{root}/closed")).unwrap();
    set_mode(&format!("{root}/closed"), 0o555);

    let refused = mkdir(&format!("{root}/closed/new")).unwrap_err();
    assert_eq!(refused.code, ErrorCode::PermissionDenied);
    assert_eq!(
        refused.message,
        format!("Permission denied: {root}/closed/new")
    );

    assert!(list(&format!("{root}/closed")).unwrap().entries.is_empty());
    set_mode(&format!("{root}/closed"), 0o755);
}

#[test]
fn refuses_a_directory_name_that_cannot_be_given() {
    let (_directory, root) = tree();
    let too_long = "n".repeat(256);

    for path in [
        format!("{root}/{too_long}"),
        format!("{root}/."),
        format!("{root}/.."),
        format!("{root}/new/"),
        format!("{root}//new"),
        format!("{root}/new\0"),
        "new".to_owned(),
        "~/new".to_owned(),
        String::new(),
    ] {
        assert_eq!(code(mkdir(&path)), ErrorCode::BadRequest, "{path:?}");
    }

    assert!(list(&root).unwrap().entries.is_empty());
}
