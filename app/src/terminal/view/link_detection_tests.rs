use std::iter;
use std::path::{Path, PathBuf};

use warp_util::path::{CleanPathResult, LineAndColumnArg};

use super::super::TerminalView;
use super::{
    GridHighlightedLink, existing_file_of_hyperlink, local_file_of_hyperlink,
    path_without_trailing_sentence_punctuation,
};
use crate::terminal::model::grid::grid_handler::PossiblePath;
use crate::terminal::model::index::Point;
use crate::terminal::model::terminal_model::WithinModel;
use crate::test_util::mock_blockgrid;
use warp_terminal::model::grid::Dimensions;

#[test]
fn strips_only_sentence_periods() {
    // A trailing period after a real file name is sentence punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md.").map(|trimmed| trimmed.path),
        Some("notes/README.md")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation(".gitignore.").map(|trimmed| trimmed.path),
        Some(".gitignore")
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("C:/Users/c/warp-md-test.md.")
            .map(|trimmed| trimmed.path),
        Some("C:/Users/c/warp-md-test.md")
    );

    // No trailing period -> nothing to trim.
    assert_eq!(
        path_without_trailing_sentence_punctuation("notes/README.md").map(|trimmed| trimmed.path),
        None
    );

    // `.`/`..` path components must be preserved, not treated as punctuation.
    assert_eq!(
        path_without_trailing_sentence_punctuation(".").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/.").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo/..").map(|trimmed| trimmed.path),
        None
    );
    assert_eq!(
        path_without_trailing_sentence_punctuation("foo..").map(|trimmed| trimmed.path),
        None
    );
}

#[test]
fn strips_trailing_fullwidth_sentence_punctuation() {
    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md，")
        .expect("fullwidth comma should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 2);

    let trimmed = path_without_trailing_sentence_punctuation("notes/README.md。！？")
        .expect("CJK sentence punctuation should be stripped");
    assert_eq!(trimmed.path, "notes/README.md");
    assert_eq!(trimmed.removed_width, 6);
}

// Regression test for https://github.com/warpdotdev/warp/issues/11477:
// a `.md` path at the end of a sentence captured the trailing period, so the
// resolved file and the highlight range ended in `.md.` and the file failed
// markdown classification. The trailing period must be excluded from both.
#[test]
fn compute_valid_paths_excludes_trailing_sentence_period() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    // The captured token as it would appear in `Drafted at <abs path>.`
    let token = format!("{}.", file.to_string_lossy());
    let end_col = token.chars().count() - 1;
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    // The resolved file excludes the trailing period (so it classifies as `.md`)...
    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    // ...and the highlighted range stops before the trailing period.
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - 1,
        }
    );
}

#[test]
fn compute_valid_paths_excludes_trailing_fullwidth_sentence_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = format!("{}，", file.to_string_lossy());
    let punctuation_width = 2;
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the markdown file should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col - punctuation_width,
        }
    );
}

#[test]
fn compute_valid_paths_keeps_trailing_fullwidth_punctuation_when_it_is_the_filename() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("warp-md-test.md，");
    std::fs::write(&file, "# Hello\n").unwrap();

    let token = file.to_string_lossy().to_string();
    let end_col = token.chars().count();
    let candidate = WithinModel::AltScreen(PossiblePath {
        path: CleanPathResult {
            path: token,
            line_and_column_num: None,
        },
        range: Point { row: 0, col: 0 }..=Point {
            row: 0,
            col: end_col,
        },
    });

    let link = TerminalView::compute_valid_paths(
        dir.path().to_str().unwrap(),
        iter::once(candidate),
        1000,
        None,
    )
    .expect("the file with fullwidth punctuation should be detected as a link");

    let GridHighlightedLink::File(file_link) = link else {
        panic!("expected a file link");
    };
    let file_link = file_link.get_inner();

    assert_eq!(
        file_link.absolute_path.file_name().unwrap(),
        "warp-md-test.md，"
    );
    assert_eq!(
        *file_link.link.range().end(),
        Point {
            row: 0,
            col: end_col,
        }
    );
}

fn line_and_column(line_num: usize, column_num: Option<usize>) -> Option<LineAndColumnArg> {
    Some(LineAndColumnArg {
        line_num,
        column_num,
    })
}

#[cfg(unix)]
#[test]
fn a_file_hyperlink_names_a_local_path() {
    use std::path::PathBuf;

    let path = |uri| local_file_of_hyperlink(uri, Some("studio")).map(|(path, _)| path);
    assert_eq!(
        path("file:///Users/ada/notes/NOW.md"),
        Some(PathBuf::from("/Users/ada/notes/NOW.md"))
    );
    assert_eq!(
        path("file:///Users/ada/some%20file%20%C5%9F.md"),
        Some(PathBuf::from("/Users/ada/some file ş.md"))
    );
    assert_eq!(
        path("file://localhost/etc/hosts"),
        Some(PathBuf::from("/etc/hosts"))
    );
    // This machine, as it names itself and as Bonjour names it.
    assert_eq!(
        path("file://studio/etc/hosts"),
        Some(PathBuf::from("/etc/hosts"))
    );
    assert_eq!(
        path("file://Studio.local/etc/hosts"),
        Some(PathBuf::from("/etc/hosts"))
    );
}

#[cfg(unix)]
#[test]
fn a_hyperlink_to_another_host_or_scheme_is_not_a_local_file() {
    assert_eq!(
        local_file_of_hyperlink("file://build-server/etc/hosts", Some("studio")),
        None
    );
    assert_eq!(
        local_file_of_hyperlink("file://studio/etc/hosts", None),
        None
    );
    assert_eq!(
        local_file_of_hyperlink("https://example.com/etc/hosts", Some("studio")),
        None
    );
    assert_eq!(
        local_file_of_hyperlink("vscode://file/etc/hosts", Some("studio")),
        None
    );
    assert_eq!(local_file_of_hyperlink("/etc/hosts", Some("studio")), None);
    assert_eq!(local_file_of_hyperlink("not a url", Some("studio")), None);
}

#[cfg(unix)]
#[test]
fn a_file_hyperlink_can_point_at_a_line_and_column() {
    let position = |uri| local_file_of_hyperlink(uri, None).and_then(|(_, position)| position);
    assert_eq!(position("file:///src/main.rs"), None);
    assert_eq!(
        position("file:///src/main.rs#L10"),
        line_and_column(10, None)
    );
    assert_eq!(
        position("file:///src/main.rs#L10C3"),
        line_and_column(10, Some(3))
    );
    assert_eq!(
        position("file:///src/main.rs#10"),
        line_and_column(10, None)
    );
    assert_eq!(
        position("file:///src/main.rs#L10-L20"),
        line_and_column(10, None)
    );
    assert_eq!(
        position("file:///src/main.rs?line=7&column=2"),
        line_and_column(7, Some(2))
    );
    assert_eq!(position("file:///src/main.rs#heading"), None);
    assert_eq!(position("file:///src/main.rs#L0"), None);
    // The fragment is not part of the path.
    assert_eq!(
        local_file_of_hyperlink("file:///src/main.rs#L10", None).map(|(path, _)| path),
        Some(std::path::PathBuf::from("/src/main.rs"))
    );
}

#[cfg(unix)]
#[test]
fn only_an_existing_regular_file_is_opened_as_one() {
    let directory = tempfile::tempdir().expect("failed to create temp dir");
    let file = directory.path().join("some file.md");
    std::fs::write(&file, "# Notes\n").expect("failed to write file");
    let uri = |path: &std::path::Path| {
        url::Url::from_file_path(path)
            .expect("absolute")
            .to_string()
    };

    assert_eq!(
        existing_file_of_hyperlink(&format!("{}#L5", uri(&file)), None),
        Some((file.clone(), line_and_column(5, None)))
    );
    assert!(uri(&file).contains("some%20file.md"));
    assert_eq!(
        existing_file_of_hyperlink(&uri(directory.path()), None),
        None
    );
    assert_eq!(
        existing_file_of_hyperlink(&uri(&directory.path().join("gone.md")), None),
        None
    );
}

/// The file, and line, that a hover on the middle of `needle` on `row` of the screen highlights
/// for a click to open, with relative paths taken from `cwd`. Rows are separated by `\n` where
/// the terminal wrapped them and by `\r\n` where the program broke the line.
fn file_at(screen: &str, row: usize, needle: &str, cwd: &Path) -> Option<(PathBuf, Option<usize>)> {
    let line = screen.split('\n').nth(row).expect("the row exists");
    let start = line.find(needle).expect("the needle is on the row");
    let col = line[..start].chars().count() + needle.chars().count() / 2;
    let grid = mock_blockgrid(screen);
    let candidates = grid
        .grid_handler()
        .possible_file_paths_at_point(Point { row, col })
        .into_iter()
        .map(WithinModel::AltScreen);
    let link = TerminalView::compute_valid_paths(
        cwd.to_str().unwrap(),
        candidates,
        grid.grid_handler().columns(),
        None,
    )?;
    let GridHighlightedLink::File(file_link) = link else {
        return None;
    };
    let file_link = file_link.get_inner();
    Some((
        file_link.absolute_path.clone(),
        file_link
            .line_and_column_num
            .map(|line_and_column| line_and_column.line_num),
    ))
}

fn project_with(files: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for file in files {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "x\n").unwrap();
    }
    (dir, root)
}

#[test]
fn paths_in_the_shapes_tools_print_them_open_the_file() {
    let (_dir, root) = project_with(&[
        "src/main.rs",
        "my dir/file name.txt",
        "report (1).pdf",
        "@scope/pkg/index.js",
        "c++/a+b.txt",
        "\u{e7}al\u{131}\u{15f}ma/\u{f6}zet.md",
    ]);
    let abs = root.to_str().unwrap();
    let main = root.join("src/main.rs");
    let cases = [
        (format!("see {abs}/my dir/file name.txt now"), "name", root.join("my dir/file name.txt"), None),
        (format!("see {abs}/report (1).pdf now"), "report", root.join("report (1).pdf"), None),
        (format!("see {abs}/@scope/pkg/index.js"), "pkg", root.join("@scope/pkg/index.js"), None),
        (format!("see {abs}/c++/a+b.txt now"), "a+b", root.join("c++/a+b.txt"), None),
        (
            format!("see {abs}/\u{e7}al\u{131}\u{15f}ma/\u{f6}zet.md now"),
            "\u{f6}zet",
            root.join("\u{e7}al\u{131}\u{15f}ma/\u{f6}zet.md"),
            None,
        ),
        ("src/main.rs:12:5: error".to_owned(), "main", main.clone(), Some(12)),
        ("see src/main.rs, then src/main.rs.".to_owned(), "main", main.clone(), None),
        ("(see src/main.rs:12)".to_owned(), "main", main.clone(), Some(12)),
        ("edit `src/main.rs` or \"src/main.rs\"".to_owned(), "main", main.clone(), None),
        ("        modified:   src/main.rs".to_owned(), "main", main.clone(), None),
        ("-rw-r--r--@ 1 me staff 2 Oct 14 12:00 report (1).pdf".to_owned(), "report", root.join("report (1).pdf"), None),
        ("src/main.rs:12:fn main() {".to_owned(), "main", main.clone(), Some(12)),
        ("see [main](src/main.rs) here".to_owned(), "main.rs", main.clone(), None),
        ("\u{23bf}  Updated src/main.rs with 3 additions".to_owned(), "main", main.clone(), None),
    ];
    for (screen, needle, file, line) in cases {
        assert_eq!(
            file_at(&screen, 0, needle, &root),
            Some((file, line)),
            "in {screen:?}"
        );
    }
}

#[test]
fn a_path_broken_over_rows_after_indentation_opens_the_whole_file() {
    let (_dir, root) = project_with(&["very/long/directory/structure/for/a/really_long_file_name.rs"]);
    let path = root.join("very/long/directory/structure/for/a/really_long_file_name.rs");
    let path_text = path.to_str().unwrap();
    let (first, rest) = path_text.split_at(path_text.find("structure").unwrap());
    let screen = format!("\u{25cf} Edited {first}\r\n  {rest} with 2 additions");

    assert_eq!(file_at(&screen, 0, "directory", &root), Some((path.clone(), None)));
    assert_eq!(file_at(&screen, 1, "really", &root), Some((path, None)));
}

#[test]
fn rows_are_joined_only_into_a_path_that_exists() {
    let (_dir, root) = project_with(&["src/main.rs", "src/mainly.rs"]);
    let screen = "  read src/mai\r\n  nly.rs and stop";

    assert_eq!(
        file_at(screen, 0, "mai", &root),
        Some((root.join("src/mainly.rs"), None))
    );
    assert_eq!(file_at(screen, 1, "stop", &root), None);

    let screen = "  read src/main.rs\r\n  only that";
    assert_eq!(
        file_at(screen, 0, "main", &root),
        Some((root.join("src/main.rs"), None))
    );
    assert_eq!(file_at(screen, 1, "only", &root), None);
}
