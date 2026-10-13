use std::collections::HashMap;

use rangemap::RangeMap;
use unindent::Unindent as _;
use warp_editor::multiline::{MultilineStr, MultilineString};

use super::{DiffModel, compute_unified_diff};
use crate::code::editor::diff::ChangeType;

#[test]
fn test_diff_generation() {
    use warpui::App;
    App::test((), |_| async move {
        let (change_mapping, deletion_mapping) = DiffModel::compute_diff_internal(
            MultilineStr::try_new("Hello World\nThis is the second line.\nThis is the third.")
                .unwrap(),
            MultilineStr::try_new(
                "Hallo Welt\nThis is the second line.\nThis is life.\nMoar and more",
            )
            .unwrap(),
        )
        .await;
        assert_eq!(
            change_mapping,
            RangeMap::from_iter([
                (
                    0..1,
                    // The two lines share no word, so none is marked.
                    ChangeType::Replacement {
                        replaced_range: 0..1,
                        insertion: vec![],
                        deletion: vec![]
                    }
                ),
                (
                    2..4,
                    // Only the first added line is an edit of the removed one.
                    ChangeType::Replacement {
                        replaced_range: 2..3,
                        insertion: vec![8..12],
                        deletion: vec![8..17]
                    }
                )
            ])
        );
        assert!(deletion_mapping.is_empty());

        let (change_mapping, deletion_mapping) = DiffModel::compute_diff_internal(
            MultilineStr::try_new("Hello World\nThis is the second line.\nThis is the third.")
                .unwrap(),
            MultilineStr::try_new("Hello World\nThis is the third.").unwrap(),
        )
        .await;
        assert!(change_mapping.is_empty());
        assert_eq!(deletion_mapping, HashMap::from([(1, 1..2)]));

        let (change_mapping, deletion_mapping) = DiffModel::compute_diff_internal(
            MultilineStr::try_new("Hello\nWorld\n").unwrap(),
            MultilineStr::try_new("Hallo\nWorlds\n").unwrap(),
        )
        .await;

        assert_eq!(
            change_mapping,
            RangeMap::from_iter([(
                0..2,
                ChangeType::Replacement {
                    replaced_range: 0..2,
                    insertion: vec![],
                    deletion: vec![]
                }
            ),])
        );
        assert!(deletion_mapping.is_empty());
    });
}

#[test]
fn test_word_changes_of_a_multi_line_replacement_count_from_the_hunk_start() {
    use warpui::App;
    App::test((), |_| async move {
        let (change_mapping, deletion_mapping) = DiffModel::compute_diff_internal(
            MultilineStr::try_new(
                "fn main() {\n    let first = load(path);\n    let second = parse(first);\n}\n",
            )
            .unwrap(),
            MultilineStr::try_new(
                "fn main() {\n    let first = load(&path);\n    log(\"loaded\");\n    let second = parse(&first)?;\n}\n",
            )
            .unwrap(),
        )
        .await;

        // Line 1 is 29 characters with its newline and line 2 is 19, so the third added line
        // starts at 48. Each range is one inserted `&` or `?`; the unpaired line has none.
        assert_eq!(
            change_mapping,
            RangeMap::from_iter([(
                1..4,
                ChangeType::Replacement {
                    replaced_range: 1..3,
                    insertion: vec![21..22, 71..72, 78..79],
                    deletion: vec![]
                }
            )])
        );
        assert!(deletion_mapping.is_empty());
    });
}

#[test]
fn test_word_changes_of_removed_lines_count_from_the_hunk_start() {
    use warpui::App;
    App::test((), |_| async move {
        let (change_mapping, _) = DiffModel::compute_diff_internal(
            MultilineStr::try_new("alpha(one, two, three)\nbeta(four, five)\n").unwrap(),
            MultilineStr::try_new("alpha(one, two)\nbeta(four, six)\n").unwrap(),
        )
        .await;

        assert_eq!(
            change_mapping,
            RangeMap::from_iter([(
                0..2,
                ChangeType::Replacement {
                    replaced_range: 0..2,
                    insertion: vec![27..30],
                    deletion: vec![14..21, 34..38]
                }
            )])
        );
    });
}

#[test]
fn test_a_pure_insertion_has_no_word_changes() {
    use warpui::App;
    App::test((), |_| async move {
        let (change_mapping, deletion_mapping) = DiffModel::compute_diff_internal(
            MultilineStr::try_new("first\nlast\n").unwrap(),
            MultilineStr::try_new("first\nadded one\nadded two\nlast\n").unwrap(),
        )
        .await;

        assert_eq!(
            change_mapping,
            RangeMap::from_iter([(1..3, ChangeType::Addition)])
        );
        assert!(deletion_mapping.is_empty());
    });
}

#[test]
fn test_reverse_action() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        diff_model.set_base(MultilineString::apply(
            "Hello World\nThis is the second line\n",
        ));
        diff_model
            .compute_diff_for_test("Hallo World\nThis is the second line\nNew".to_string())
            .await;

        assert_eq!(diff_model.diff_hunk_count(), 2);
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(0),
            Some((0..1, "Hello World\n".to_string()))
        );

        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(1),
            Some((2..3, "".to_string()))
        );

        diff_model
            .compute_diff_for_test("Hello World\n".to_string())
            .await;
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(0),
            Some((1..1, "This is the second line\n".to_string()))
        );
    });
}

#[test]
fn test_reverse_action_replaced_newlines() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        let base_text = r"

            abc
            def

            ghi

            jkl
            mno


            pqr

            stu

        "
        .unindent();
        diff_model.set_base(MultilineString::apply(&base_text));

        // Replace with text:
        // * Leading newline before "abc"
        // * "def" and the following line
        // * Newline between "def" and "ghi"
        // * "pqr" and the preceding line
        // * Trailing newline at end of file
        let modified_text = r"
            replaced first empty line
            abc
            changed def
            changed line after def
            ghi
            changed line between ghi and jkl
            jkl
            mno

            changed line before pqr
            changed pqr

            stu
            replaced last empty line
        "
        .unindent();
        diff_model.compute_diff_for_test(modified_text).await;

        assert_eq!(diff_model.diff_hunk_count(), 5);

        // Reversing the leading newline change
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(0),
            Some((0..1, "\n".to_string()))
        );

        // Reversing "def" and following newline
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(1),
            Some((2..4, "def\n\n".to_string()))
        );

        // Reversing the line changed between "ghi" and "jkl"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(2),
            Some((5..6, "\n".to_string()))
        );

        // Reversing changing "pqr" and the line before it
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(3),
            Some((9..11, "\npqr\n".to_string()))
        );

        // Reversing changing the last line
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(4),
            Some((13..14, "\n".to_string()))
        );
    });
}

#[test]
fn test_reverse_action_replaced_text() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        let base_text = r"
            abc
            def
            ghi

            jkl
            mno
            pqr

            stu
            vwx
            yz
        "
        .unindent();
        diff_model.set_base(MultilineString::apply(&base_text));

        // Replace with a newline:
        // * First line "abc"
        // * "ghi", which is followed by a newline
        // * "mno"
        // * "stu", which is preceded by a newline
        // * Last line "yz"
        let modified_text = r"

            def


            jkl

            pqr


            vwx

        "
        .unindent();
        diff_model.compute_diff_for_test(modified_text).await;

        assert_eq!(diff_model.diff_hunk_count(), 5);

        // Reversing the leading newline change
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(0),
            Some((0..1, "abc\n".to_string()))
        );

        // Reversing "ghi"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(1),
            Some((3..4, "ghi\n".to_string()))
        );

        // Reversing "mno"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(2),
            Some((5..6, "mno\n".to_string()))
        );

        // Reversing "stu"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(3),
            Some((8..9, "stu\n".to_string()))
        );

        // Reversing changing the last line
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(4),
            Some((10..11, "yz\n".to_string()))
        );
    });
}

#[test]
fn test_reverse_action_deleted_lines() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        let base_text = r"

            abc
            def

            ghi

            jkl
            mno


            pqr


            stu
            vwx

            yz

        "
        .unindent();
        diff_model.set_base(MultilineString::apply(&base_text));

        // Delete:
        // * Leading newline before "abc"
        // * Newline between "def" and "ghi"
        // * "mno" followed by a newline
        // * newline followed by "stu"
        // * Trailing newline after "yz"
        let modified_text = r"
            abc
            def
            ghi

            jkl

            pqr

            vwx

            yz
        "
        .unindent();
        diff_model.compute_diff_for_test(modified_text).await;

        assert_eq!(diff_model.diff_hunk_count(), 5);

        // Reversing the leading newline deletion
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(0),
            Some((0..0, "\n".to_string()))
        );

        // Reversing the newline deletion between "def" and "ghi"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(1),
            Some((2..2, "\n".to_string()))
        );

        // Reversing the deletion of "mno" followed by a newline
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(2),
            Some((5..5, "mno\n\n".to_string()))
        );

        // Reversing the deletion of a newline followed by "stu"
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(3),
            Some((8..8, "\nstu\n".to_string()))
        );

        // Reversing the trailing newline deletion
        assert_eq!(
            diff_model.reverse_action_by_diff_hunk_index(4),
            Some((11..11, "\n".to_string()))
        );
    });
}

#[test]
fn test_diff_count_before_line() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        diff_model.set_base(
            "Hello World\nThis is the second line\n"
                .to_owned()
                .try_into()
                .unwrap(),
        );
        diff_model
            .compute_diff_for_test("Hallo World\nThis is the second line\nNew".to_string())
            .await;
        assert_eq!(diff_model.diff_hunk_count_before_line(0), 0);
        assert_eq!(diff_model.diff_hunk_count_before_line(1), 1);
        assert_eq!(diff_model.diff_hunk_count_before_line(2), 1);
    });
}

#[test]
fn test_unified_diff() {
    use warpui::App;
    App::test((), |_| async move {
        let diff = compute_unified_diff(
            MultilineStr::try_new("Hello World\nThis is the second line.\nThis is the third.")
                .unwrap(),
            MultilineStr::try_new(
                "Hallo Welt\nThis is the second line.\nThis is life.\nMoar and more",
            )
            .unwrap(),
            "test.rs",
        )
        .await;
        assert_eq!(
            diff.unified_diff,
            "--- test.rs\n+++ test.rs\n@@ -1,3 +1,4 @@\n-Hello World\n+Hallo Welt\n This is the second line.\n-This is the third.\n+This is life.\n+Moar and more\n"
        );
        assert_eq!(diff.lines_added, 3);
        assert_eq!(diff.lines_removed, 2);
    });
}

/// Test coalesce_replacements with a case where the `similar` library is known
/// to produce duplicate deletion and insertion hunks for what is logically a replacement.
#[test]
fn test_coalesce_replacements() {
    use warpui::App;
    App::test((), |_| async move {
        let mut diff_model = DiffModel::new();
        let base_text = r"
            abc
            def
            ghi

            jkl
            mno
            pqr

            stu
            vwx
            yz
        "
        .unindent();
        diff_model.set_base(MultilineString::apply(&base_text));

        // Replace with a newline:
        // * First line "abc"
        // * "ghi", which is followed by a newline
        // * "mno"
        // * "stu", which is preceded by a newline
        // * Last line "yz"
        let modified_text = r"

            def


            jkl

            pqr


            vwx

        "
        .unindent();
        diff_model.compute_diff_for_test(modified_text).await;

        assert_eq!(diff_model.diff_hunk_count(), 5);

        // Replacing "abc"
        assert_eq!(diff_model.diff_by_index(0), Some((0..1, true)));

        // Replacing "ghi"
        assert_eq!(diff_model.diff_by_index(1), Some((3..4, true)));

        // Replacing "mno"
        assert_eq!(diff_model.diff_by_index(2), Some((5..6, true)));

        // Replacing "stu"
        assert_eq!(diff_model.diff_by_index(3), Some((8..9, true)));

        // Replacing "yz"
        assert_eq!(diff_model.diff_by_index(4), Some((10..11, true)));
    });
}

#[test]
fn test_a_skipped_diff_reports_no_differences_and_says_it_is_done() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use warp_editor::content::version::BufferVersion;
    use warpui::App;

    use super::DiffModelEvent;

    App::test((), |mut app| async move {
        let mut diff_model = DiffModel::new();
        diff_model.set_base(MultilineString::apply("one\ntwo\n"));
        diff_model
            .compute_diff_for_test("one\nTWO\nthree\n".to_string())
            .await;
        assert!(diff_model.diff_hunk_count() > 0);
        let diff_model = app.add_model(|_| diff_model);

        let reported = Rc::new(RefCell::new(Vec::new()));
        let versions = reported.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&diff_model, move |_, event, _| match event {
                DiffModelEvent::DiffUpdated { version } => versions.borrow_mut().push(*version),
                DiffModelEvent::UnifiedDiffComputed(_) => {}
            });
        });
        let version = BufferVersion::new();
        diff_model.update(&mut app, |diff_model, ctx| {
            diff_model.skip_diff(version, ctx)
        });

        diff_model.read(&app, |diff_model, _| {
            assert_eq!(diff_model.diff_hunk_count(), 0);
            // The base stays, for the day the buffer is small enough to compare again.
            assert!(diff_model.has_base());
        });
        assert_eq!(*reported.borrow(), [version]);
    });
}
