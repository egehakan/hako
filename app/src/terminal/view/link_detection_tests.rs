use std::iter;

use warp_util::path::{CleanPathResult, LineAndColumnArg};

use super::super::TerminalView;
use super::{
    GridHighlightedLink, existing_file_of_hyperlink, local_file_of_hyperlink,
    path_without_trailing_sentence_punctuation,
};
use crate::terminal::model::grid::grid_handler::PossiblePath;
use crate::terminal::model::index::Point;
use crate::terminal::model::terminal_model::WithinModel;

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
    assert_eq!(path("file://studio/etc/hosts"), Some(PathBuf::from("/etc/hosts")));
    assert_eq!(
        path("file://Studio.local/etc/hosts"),
        Some(PathBuf::from("/etc/hosts"))
    );
}

#[cfg(unix)]
#[test]
fn a_hyperlink_to_another_host_or_scheme_is_not_a_local_file() {
    assert_eq!(local_file_of_hyperlink("file://build-server/etc/hosts", Some("studio")), None);
    assert_eq!(local_file_of_hyperlink("file://studio/etc/hosts", None), None);
    assert_eq!(local_file_of_hyperlink("https://example.com/etc/hosts", Some("studio")), None);
    assert_eq!(local_file_of_hyperlink("vscode://file/etc/hosts", Some("studio")), None);
    assert_eq!(local_file_of_hyperlink("/etc/hosts", Some("studio")), None);
    assert_eq!(local_file_of_hyperlink("not a url", Some("studio")), None);
}

#[cfg(unix)]
#[test]
fn a_file_hyperlink_can_point_at_a_line_and_column() {
    let position = |uri| local_file_of_hyperlink(uri, None).and_then(|(_, position)| position);
    assert_eq!(position("file:///src/main.rs"), None);
    assert_eq!(position("file:///src/main.rs#L10"), line_and_column(10, None));
    assert_eq!(position("file:///src/main.rs#L10C3"), line_and_column(10, Some(3)));
    assert_eq!(position("file:///src/main.rs#10"), line_and_column(10, None));
    assert_eq!(position("file:///src/main.rs#L10-L20"), line_and_column(10, None));
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
    let uri = |path: &std::path::Path| url::Url::from_file_path(path).expect("absolute").to_string();

    assert_eq!(
        existing_file_of_hyperlink(&format!("{}#L5", uri(&file)), None),
        Some((file.clone(), line_and_column(5, None)))
    );
    assert!(uri(&file).contains("some%20file.md"));
    assert_eq!(existing_file_of_hyperlink(&uri(directory.path()), None), None);
    assert_eq!(
        existing_file_of_hyperlink(&uri(&directory.path().join("gone.md")), None),
        None
    );
}
