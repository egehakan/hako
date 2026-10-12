use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use repo_metadata::RepoMetadataModel;
use virtual_fs::VirtualFS;
use warp_core::telemetry::testing::MockTelemetryContextProvider;
use warpui::r#async::Timer;
use warpui::clipboard::ClipboardContent;
use warpui::platform::WindowStyle;
use warpui::{App, ModelHandle, SingletonEntity, TypedActionView, ViewHandle};

use super::super::view_tests::{
    await_directory_loaded, await_repository_indexed, initialize_app, item_id, record_opened_files,
    std_path,
};
use super::super::{FileTreeAction, FileTreeEvent, FileTreeView, PendingEditKind};
use crate::code::editor_management::TabOpenMode;
use crate::code::file_tree::trash::TEST_TRASH_DIR;
use crate::workspace::{ToastStack, ToastStackEvent};

/// What the tree told the workspace about files that moved or went away.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reported {
    Renamed(PathBuf, PathBuf),
    Deleted(PathBuf),
}

struct Fixture {
    tree: PathBuf,
    view: ViewHandle<FileTreeView>,
    repository_metadata_model: ModelHandle<RepoMetadataModel>,
    reported: Rc<RefCell<Vec<Reported>>>,
    /// The messages shown about operations that went wrong.
    errors: Rc<RefCell<Vec<String>>>,
    /// The messages shown about operations that went through.
    messages: Rc<RefCell<Vec<String>>>,
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

/// Shows `tree` in a file tree and records what the tree reports and the errors it shows.
async fn open_tree(app: &mut App, tree: &Path) -> Fixture {
    let (_, repository_metadata_model) = initialize_app(app);
    // Creating an item is reported to telemetry.
    app.update(MockTelemetryContextProvider::register);
    let (_, view) = app.add_window(WindowStyle::NotStealFocus, FileTreeView::new);
    view.update(app, |view, ctx| {
        view.set_is_active(true, ctx);
        view.set_root_directories(vec![tree.to_path_buf()], ctx);
    });
    await_repository_indexed(app, &repository_metadata_model, tree).await;

    let reported = Rc::new(RefCell::new(Vec::new()));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let messages = Rc::new(RefCell::new(Vec::new()));
    app.update(|ctx| {
        let recorded = reported.clone();
        ctx.subscribe_to_view(&view, move |_, event, _| match event {
            FileTreeEvent::FileRenamed { old_path, new_path } => recorded
                .borrow_mut()
                .push(Reported::Renamed(old_path.clone(), new_path.clone())),
            FileTreeEvent::FileDeleted { path } => {
                recorded.borrow_mut().push(Reported::Deleted(path.clone()))
            }
            FileTreeEvent::AttachAsContext { .. }
            | FileTreeEvent::OpenFile { .. }
            | FileTreeEvent::CDToDirectory { .. }
            | FileTreeEvent::OpenDirectoryInNewTab { .. }
            | FileTreeEvent::SearchInFolder { .. } => {}
        });
        let (recorded_errors, recorded_messages) = (errors.clone(), messages.clone());
        ctx.subscribe_to_model(&ToastStack::handle(ctx), move |_, event, _| {
            if let ToastStackEvent::AddEphemeralToast { toast, .. } = event {
                let text = toast.main_text().to_owned();
                if text.starts_with("Couldn’t") {
                    recorded_errors.borrow_mut().push(text);
                } else {
                    recorded_messages.borrow_mut().push(text);
                }
            }
        });
    });

    Fixture {
        tree: tree.to_path_buf(),
        view,
        repository_metadata_model,
        reported,
        errors,
        messages,
    }
}

impl Fixture {
    fn act(&self, app: &mut App, action: FileTreeAction) {
        self.view
            .update(app, |view, ctx| view.handle_action(&action, ctx));
    }

    fn select(&self, app: &mut App, path: &Path) {
        let id = item_id(app, &self.view, &self.tree, path);
        self.view.update(app, |view, ctx| view.select_id(&id, ctx));
    }

    fn toggle(&self, app: &mut App, path: &Path) {
        let id = item_id(app, &self.view, &self.tree, path);
        self.act(app, FileTreeAction::ToggleItemSelection { id });
    }

    /// Expands the folder at `path` and waits for its contents.
    async fn expand(&self, app: &mut App, path: &Path) {
        self.view.update(app, |view, ctx| {
            view.toggle_folder_expansion(&std_path(&self.tree), &std_path(path), ctx);
        });
        await_directory_loaded(app, &self.repository_metadata_model, &self.tree, path).await;
    }

    fn selected_paths(&self, app: &App) -> Vec<PathBuf> {
        self.view.read(app, |view, _ctx| {
            view.selected_ids()
                .iter()
                .filter_map(|id| view.item_at(id))
                .map(|item| item.path().to_local_path_lossy())
                .collect()
        })
    }

    fn is_shown(&self, app: &App, path: &Path) -> bool {
        let path = std_path(path);
        self.view.read(app, |view, _ctx| {
            view.root_directories[&std_path(&self.tree)]
                .items
                .iter()
                .any(|item| item.path() == &path)
        })
    }

    fn clipboard(&self, app: &mut App) -> ClipboardContent {
        app.update(|ctx| ctx.clipboard().read())
    }

    /// Waits for the background work the tree started to make `condition` true.
    async fn wait_until(
        &self,
        app: &mut App,
        what: &str,
        condition: impl Fn(&FileTreeView) -> bool,
    ) {
        for _ in 0..500 {
            if self.view.read(app, |view, _ctx| condition(view)) {
                return;
            }
            Timer::after(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting until {what}");
    }

    /// Waits until the tree is about to select `path`, or has.
    async fn wait_until_revealed(&self, app: &mut App, path: &Path) {
        let path = std_path(path);
        self.wait_until(app, "the new item is revealed", |view| {
            view.selected_item_std_path().as_ref() == Some(&path)
                || view
                    .pending_focus_target
                    .as_ref()
                    .is_some_and(|target| target.path == path)
        })
        .await;
    }
}

fn file_paths(paths: &[&Path]) -> ClipboardContent {
    let paths: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    ClipboardContent {
        plain_text: paths.join("\n"),
        paths: Some(paths),
        ..Default::default()
    }
}

#[test]
fn modifier_clicks_select_several_items() {
    VirtualFS::test("file_tree_multi_selection", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let [a, b, c, d] = ["a.txt", "b.txt", "c.txt", "d.txt"].map(|name| tree.join(name));
        for file in [&a, &b, &c, &d] {
            write(file, "x");
        }

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.toggle(&mut app, &c);
            assert_eq!(fixture.selected_paths(&app), [a.clone(), c.clone()]);

            // Taking an item out again leaves a plain single selection.
            fixture.toggle(&mut app, &a);
            assert_eq!(fixture.selected_paths(&app), [c.clone()]);
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.multi_selection.is_empty()));

            // A shift-click selects everything from the item last added through the clicked one.
            let a_id = item_id(&app, &fixture.view, &tree, &a);
            fixture.act(&mut app, FileTreeAction::ExtendSelectionTo { id: a_id });
            assert_eq!(
                fixture.selected_paths(&app),
                [a.clone(), b.clone(), c.clone()]
            );

            fixture.act(&mut app, FileTreeAction::SelectNextItem);
            assert_eq!(fixture.selected_paths(&app).len(), 1);

            fixture.select(&mut app, &b);
            fixture.toggle(&mut app, &d);
            fixture.act(&mut app, FileTreeAction::ClearCutAndSelection);
            assert_eq!(fixture.selected_paths(&app).len(), 1);
        });
    });
}

#[test]
fn copy_and_cut_put_the_selected_files_on_the_clipboard() {
    VirtualFS::test("file_tree_copy_cut", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let nested = tree.join("pkg/nested.txt");
        let pkg = tree.join("pkg");
        write(&a, "a");
        write(&nested, "nested");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            fixture.expand(&mut app, &pkg).await;

            // A file selected along with the folder it is in is covered by the folder.
            fixture.select(&mut app, &a);
            fixture.toggle(&mut app, &pkg);
            fixture.toggle(&mut app, &nested);
            fixture.act(&mut app, FileTreeAction::CopySelection);

            // In the order the tree shows them: folders come first.
            let expected = [pkg.clone(), a.clone()].map(|path| path.to_string_lossy().into_owned());
            let clipboard = fixture.clipboard(&mut app);
            assert_eq!(clipboard.paths.as_deref(), Some(expected.as_slice()));
            assert_eq!(clipboard.plain_text, expected.join("\n"));
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.cut_paths.is_empty()));

            fixture.act(&mut app, FileTreeAction::CutSelection);
            fixture.view.read(&app, |view, _ctx| {
                assert!(view.cut_paths.contains(&std_path(&a)));
                assert!(view.cut_paths.contains(&std_path(&pkg)));
                assert_eq!(view.cut_paths.len(), 2);
            });
            assert_eq!(
                fixture.clipboard(&mut app).paths.as_deref(),
                Some(expected.as_slice())
            );

            fixture.act(&mut app, FileTreeAction::ClearCutAndSelection);
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.cut_paths.is_empty()));

            // The root of the tree can be copied but not cut.
            fixture.select(&mut app, &tree);
            fixture.act(&mut app, FileTreeAction::CutSelection);
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.cut_paths.is_empty()));
        });
    });
}

#[test]
fn paste_copies_into_the_folder_the_selection_stands_for() {
    VirtualFS::test("file_tree_paste_copy", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let pkg = tree.join("pkg");
        write(&a, "a");
        write(&pkg.join("inner.txt"), "inner");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::CopySelection);

            // On a folder: into the folder, which opens to show the copy.
            fixture.select(&mut app, &pkg);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &pkg.join("a.txt"))
                .await;
            assert_eq!(read(&pkg.join("a.txt")), "a");
            assert_eq!(read(&a), "a");
            fixture.view.read(&app, |view, _ctx| {
                let expanded = &view.root_directories[&std_path(&tree)].expanded_folders;
                assert!(expanded.contains(&std_path(&pkg)));
            });

            // On a file: next to the file, under a copy name when the name is taken.
            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &tree.join("a copy.txt"))
                .await;
            assert_eq!(read(&tree.join("a copy.txt")), "a");
            assert!(fixture.reported.borrow().is_empty());
            assert!(fixture.errors.borrow().is_empty());
        });
    });
}

#[test]
fn paste_after_a_cut_moves_the_items_once() {
    VirtualFS::test("file_tree_paste_cut", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let b = tree.join("b.txt");
        let pkg = tree.join("pkg");
        write(&a, "a");
        write(&b, "b");
        write(&pkg.join("inner.txt"), "inner");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.toggle(&mut app, &b);
            fixture.act(&mut app, FileTreeAction::CutSelection);
            fixture.select(&mut app, &pkg);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &pkg.join("a.txt"))
                .await;

            assert!(!a.exists());
            assert!(!b.exists());
            assert_eq!(read(&pkg.join("a.txt")), "a");
            assert_eq!(read(&pkg.join("b.txt")), "b");
            // Both moved items end up selected, and their editors are told where they went.
            fixture.view.read(&app, |view, _ctx| {
                assert!(view.cut_paths.is_empty());
                assert!(view.multi_selection.contains(&std_path(&pkg.join("a.txt"))));
                assert!(view.multi_selection.contains(&std_path(&pkg.join("b.txt"))));
            });
            assert_eq!(
                *fixture.reported.borrow(),
                [
                    Reported::Renamed(a.clone(), pkg.join("a.txt")),
                    Reported::Renamed(b.clone(), pkg.join("b.txt")),
                ]
            );
            // The moved items leave the tree without waiting for it to notice.
            assert!(!fixture.is_shown(&app, &a));
            assert!(!fixture.is_shown(&app, &b));

            // The cut is spent: pasting again is a copy of files that are no longer there.
            fixture.select(&mut app, &tree);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until(&mut app, "the failed paste is reported", |_| {
                    !fixture.errors.borrow().is_empty()
                })
                .await;
            assert!(fixture.errors.borrow()[0].starts_with("Couldn’t copy 2 items."));
            assert!(!a.exists());
        });
    });
}

#[test]
fn paste_of_files_copied_in_another_app_copies_them_even_after_a_cut() {
    VirtualFS::test("file_tree_paste_external", |dirs, mut vfs| {
        vfs.mkdir("tree");
        vfs.mkdir("elsewhere");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let outside = dirs.tests().join("elsewhere/from_finder.txt");
        let outside_folder = dirs.tests().join("elsewhere/assets");
        write(&a, "a");
        write(&outside, "finder");
        write(&outside_folder.join("logo.svg"), "<svg/>");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::CutSelection);
            // Something else is copied in another app before the paste.
            app.update(|ctx| {
                ctx.clipboard()
                    .write(file_paths(&[&outside, &outside_folder]));
            });

            fixture.select(&mut app, &tree);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &tree.join("from_finder.txt"))
                .await;

            assert_eq!(read(&tree.join("from_finder.txt")), "finder");
            assert_eq!(read(&tree.join("assets/logo.svg")), "<svg/>");
            assert_eq!(read(&outside), "finder");
            assert!(outside_folder.join("logo.svg").exists());
            // The cut item stays where it was, and is no longer cut.
            assert_eq!(read(&a), "a");
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.cut_paths.is_empty()));
            assert!(fixture.reported.borrow().is_empty());
        });
    });
}

#[test]
fn folder_pasted_into_itself_is_refused_with_a_message() {
    VirtualFS::test("file_tree_paste_into_itself", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let pkg = tree.join("pkg");
        let inner = pkg.join("inner");
        write(&inner.join("deep.txt"), "deep");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            fixture.expand(&mut app, &pkg).await;

            fixture.select(&mut app, &pkg);
            fixture.act(&mut app, FileTreeAction::CopySelection);
            fixture.select(&mut app, &inner);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until(&mut app, "the refusal is reported", |_| {
                    !fixture.errors.borrow().is_empty()
                })
                .await;

            assert_eq!(
                *fixture.errors.borrow(),
                ["Couldn’t copy “pkg”: a folder can’t be copied into itself"]
            );
            assert!(!inner.join("pkg").exists());
        });
    });
}

#[test]
fn dropped_items_move_and_files_dropped_from_another_app_are_copied() {
    VirtualFS::test("file_tree_drops", |dirs, mut vfs| {
        vfs.mkdir("tree");
        vfs.mkdir("elsewhere");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let b = tree.join("b.txt");
        let pkg = tree.join("pkg");
        let inner = pkg.join("inner.txt");
        let outside = dirs.tests().join("elsewhere/dropped.txt");
        write(&a, "a");
        write(&b, "b");
        write(&inner, "inner");
        write(&outside, "dropped");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            fixture.expand(&mut app, &pkg).await;
            let id = |app: &App, path: &Path| item_id(app, &fixture.view, &tree, path);

            // Onto a file: into the folder that holds the file.
            let (a_id, inner_id) = (id(&app, &a), id(&app, &inner));
            fixture.act(
                &mut app,
                FileTreeAction::ItemDroppedOnItem {
                    id: a_id,
                    target: inner_id,
                },
            );
            fixture
                .wait_until_revealed(&mut app, &pkg.join("a.txt"))
                .await;
            assert!(!a.exists());
            assert_eq!(read(&pkg.join("a.txt")), "a");
            assert_eq!(
                *fixture.reported.borrow(),
                [Reported::Renamed(a.clone(), pkg.join("a.txt"))]
            );

            // An item dropped on itself stays put, without a message.
            let b_id = id(&app, &b);
            fixture.act(
                &mut app,
                FileTreeAction::ItemDroppedOnItem {
                    id: b_id.clone(),
                    target: b_id,
                },
            );
            assert!(b.exists());

            let pkg_id = id(&app, &pkg);
            fixture.act(
                &mut app,
                FileTreeAction::ExternalFilesDropped {
                    paths: vec![outside.to_string_lossy().into_owned()],
                    target: Some(pkg_id),
                },
            );
            fixture
                .wait_until_revealed(&mut app, &pkg.join("dropped.txt"))
                .await;
            assert_eq!(read(&pkg.join("dropped.txt")), "dropped");
            assert_eq!(read(&outside), "dropped");

            // Outside every item: into the root of the tree.
            fixture.act(
                &mut app,
                FileTreeAction::ExternalFilesDropped {
                    paths: vec![outside.to_string_lossy().into_owned()],
                    target: None,
                },
            );
            fixture
                .wait_until_revealed(&mut app, &tree.join("dropped.txt"))
                .await;
            assert_eq!(read(&tree.join("dropped.txt")), "dropped");
            assert!(fixture.errors.borrow().is_empty());
        });
    });
}

#[test]
fn trashing_takes_the_row_away_at_once_and_puts_it_back_on_failure() {
    VirtualFS::test("file_tree_trash", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let stuck = tree.join("untrashable.txt");
        write(&a, "a");
        write(&stuck, "stuck");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::DeleteSelection);
            // Gone from the tree before the disk has been touched.
            assert!(!fixture.is_shown(&app, &a));
            fixture
                .wait_until(&mut app, "the item is trashed", |view| {
                    view.paths_being_trashed.is_empty()
                })
                .await;
            assert!(!a.exists());
            assert_eq!(read(&tree.join(TEST_TRASH_DIR).join("a.txt")), "a");
            assert_eq!(*fixture.reported.borrow(), [Reported::Deleted(a.clone())]);
            assert!(!fixture.is_shown(&app, &a));
            // Keyboard navigation continues from the folder the item was in.
            assert_eq!(fixture.selected_paths(&app), [tree.clone()]);

            fixture.select(&mut app, &stuck);
            fixture.act(&mut app, FileTreeAction::DeleteSelection);
            assert!(!fixture.is_shown(&app, &stuck));
            fixture
                .wait_until(&mut app, "the failure is reported", |view| {
                    view.paths_being_trashed.is_empty()
                })
                .await;
            assert!(stuck.exists());
            assert!(fixture.is_shown(&app, &stuck));
            assert_eq!(
                *fixture.errors.borrow(),
                ["Couldn’t move to the Trash “untrashable.txt”: it is in use"]
            );
        });
    });
}

#[test]
fn trashing_asks_first_for_several_items_and_for_folders_that_may_hold_something() {
    VirtualFS::test("file_tree_trash_confirmation", |dirs, mut vfs| {
        vfs.mkdir("tree/empty");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let b = tree.join("b.txt");
        let empty = tree.join("empty");
        let full = tree.join("full");
        let unloaded = tree.join("unloaded");
        write(&a, "a");
        write(&b, "b");
        write(&full.join("inner.txt"), "inner");
        write(&unloaded.join("inner.txt"), "inner");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            fixture.expand(&mut app, &empty).await;
            fixture.expand(&mut app, &full).await;
            let id = |path: &Path| item_id(&app, &fixture.view, &tree, path);
            let (a_id, b_id) = (id(&a), id(&b));
            let (empty_id, full_id, unloaded_id) = (id(&empty), id(&full), id(&unloaded));

            fixture.view.read(&app, |view, _ctx| {
                assert!(!view.needs_trash_confirmation(std::slice::from_ref(&a_id)));
                assert!(!view.needs_trash_confirmation(std::slice::from_ref(&empty_id)));
                assert!(view.needs_trash_confirmation(std::slice::from_ref(&full_id)));
                // What a folder holds is only known once the tree has loaded it.
                assert!(view.needs_trash_confirmation(std::slice::from_ref(&unloaded_id)));
                assert!(view.needs_trash_confirmation(&[a_id.clone(), b_id.clone()]));
            });

            // Nothing is touched until the confirmation is answered.
            fixture.select(&mut app, &a);
            fixture.toggle(&mut app, &b);
            fixture.act(&mut app, FileTreeAction::DeleteSelection);
            assert!(fixture.is_shown(&app, &a));
            assert!(fixture.is_shown(&app, &b));
            assert!(a.exists() && b.exists());

            // The root of the tree is never trashed.
            fixture.select(&mut app, &tree);
            fixture.act(&mut app, FileTreeAction::DeleteSelection);
            assert!(fixture.is_shown(&app, &tree));
        });
    });
}

impl Fixture {
    /// Types `name` into the name field of the item being created or renamed and confirms it.
    fn confirm_name(&self, app: &mut App, name: &str) {
        self.view.update(app, |view, ctx| {
            view.editor_view
                .update(ctx, |editor, ctx| editor.set_buffer_text(name, ctx));
            view.commit_pending_edit(true, ctx);
        });
    }

    async fn wait_until_created(&self, app: &mut App) {
        self.wait_until(app, "the disk has answered", |view| !view.is_creating)
            .await;
    }
}

#[test]
fn new_folder_is_created_in_the_folder_of_the_item_it_was_asked_for_on() {
    VirtualFS::test("file_tree_new_folder", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let inner = tree.join("pkg/inner.txt");
        write(&inner, "inner");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            fixture.expand(&mut app, &tree.join("pkg")).await;

            let inner_id = item_id(&app, &fixture.view, &tree, &inner);
            fixture.act(&mut app, FileTreeAction::NewFolder { id: inner_id });
            fixture.view.read(&app, |view, _ctx| {
                assert_eq!(
                    view.pending_edit.as_ref().map(|edit| edit.kind),
                    Some(PendingEditKind::CreateNewFolder)
                );
            });
            fixture.confirm_name(&mut app, "made");
            fixture.wait_until_created(&mut app).await;

            let made = tree.join("pkg/made");
            assert!(made.is_dir());
            // The new folder is shown, selected and open.
            assert!(fixture.is_shown(&app, &made));
            assert_eq!(fixture.selected_paths(&app), [made.clone()]);
            fixture.view.read(&app, |view, _ctx| {
                assert!(view.pending_edit.is_none());
                let expanded = &view.root_directories[&std_path(&tree)].expanded_folders;
                assert!(expanded.contains(&std_path(&made)));
            });
            assert!(fixture.errors.borrow().is_empty());
            assert_eq!(*fixture.messages.borrow(), ["Created “made”"]);

            // Undone while it is still empty.
            fixture.act(&mut app, FileTreeAction::Undo);
            fixture
                .wait_until(&mut app, "the folder is trashed", |view| {
                    !view.is_reversing && view.redo_stack.len() == 1
                })
                .await;
            assert!(!made.exists());
            assert!(!fixture.is_shown(&app, &made));
        });
    });
}

#[test]
fn new_file_with_a_nested_path_creates_the_folders_on_the_way_and_opens_it_kept() {
    VirtualFS::test("file_tree_new_nested_file", |dirs, mut vfs| {
        vfs.mkdir("tree/src");
        let tree = dirs.tests().join("tree");
        let src = tree.join("src");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;
            let opened = record_opened_files(&mut app, &fixture.view);

            // From the header buttons: in the folder the selection stands for.
            fixture.select(&mut app, &src);
            fixture.view.update(&mut app, |view, ctx| {
                view.start_creating_in_selection(false, ctx);
            });
            fixture.confirm_name(&mut app, "components/forms/LoginForm.tsx");
            fixture.wait_until_created(&mut app).await;

            let file = src.join("components/forms/LoginForm.tsx");
            assert_eq!(read(&file), "");
            assert_eq!(*opened.borrow(), [(file.clone(), TabOpenMode::Keep)]);
            fixture.wait_until_revealed(&mut app, &file).await;
            assert_eq!(*fixture.messages.borrow(), ["Created “LoginForm.tsx”"]);

            // Undoing takes the folders made on the way with it.
            fixture.act(&mut app, FileTreeAction::Undo);
            fixture
                .wait_until(&mut app, "the file is trashed", |view| {
                    !view.is_reversing && view.redo_stack.len() == 1
                })
                .await;
            assert!(!src.join("components").exists());
            assert!(src.is_dir());
            assert_eq!(
                fixture.reported.borrow().last(),
                Some(&Reported::Deleted(src.join("components")))
            );
        });
    });
}

#[test]
fn new_item_with_a_refused_name_keeps_the_name_field_open_and_creates_nothing() {
    VirtualFS::test("file_tree_new_refused", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let taken = tree.join("taken.txt");
        write(&taken, "mine");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            // Nothing selected: at the root of the tree.
            fixture.view.update(&mut app, |view, ctx| {
                view.selected_item = None;
                view.start_creating_in_selection(false, ctx);
            });
            for (name, error) in [
                (
                    "taken.txt",
                    "Couldn’t create it: “taken.txt” already exists",
                ),
                (
                    "../escape.txt",
                    "Couldn’t create it: “../escape.txt” can’t be used as a name",
                ),
                ("   ", "Couldn’t create it: a name is needed"),
            ] {
                fixture.confirm_name(&mut app, name);
                fixture.wait_until_created(&mut app).await;
                assert_eq!(
                    fixture.errors.borrow().last().map(String::as_str),
                    Some(error)
                );
                fixture.view.read(&app, |view, _ctx| {
                    assert_eq!(
                        view.pending_edit.as_ref().map(|edit| edit.kind),
                        Some(PendingEditKind::CreateNewFile)
                    );
                });
            }
            assert_eq!(read(&taken), "mine");
            assert!(!dirs_escape(&tree).exists());

            // The same field then takes a name that is free.
            fixture.confirm_name(&mut app, "free.txt");
            fixture.wait_until_created(&mut app).await;
            assert_eq!(read(&tree.join("free.txt")), "");
            fixture
                .view
                .read(&app, |view, _ctx| assert!(view.pending_edit.is_none()));
        });
    });
}

fn dirs_escape(tree: &Path) -> PathBuf {
    tree.parent().expect("tree has a parent").join("escape.txt")
}

#[test]
fn rename_onto_a_taken_name_is_refused_with_a_message() {
    VirtualFS::test("file_tree_rename_taken", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let b = tree.join("b.txt");
        write(&a, "a");
        write(&b, "b");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::RenameSelected);
            fixture.confirm_name(&mut app, "b.txt");

            assert_eq!(read(&a), "a");
            assert_eq!(read(&b), "b");
            assert_eq!(
                *fixture.errors.borrow(),
                ["Couldn’t rename: “b.txt” already exists"]
            );
            assert!(fixture.reported.borrow().is_empty());
        });
    });
}

#[test]
fn undo_and_redo_reverse_a_delete() {
    VirtualFS::test("file_tree_undo_delete", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        write(&a, "a");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::DeleteSelection);
            fixture
                .wait_until(&mut app, "the item is trashed", |view| {
                    view.undo_stack.len() == 1
                })
                .await;
            assert!(!a.exists());
            assert_eq!(*fixture.messages.borrow(), ["Moved “a.txt” to the Trash"]);

            fixture.act(&mut app, FileTreeAction::Undo);
            fixture.wait_until_revealed(&mut app, &a).await;
            assert_eq!(read(&a), "a");
            fixture.view.read(&app, |view, _ctx| {
                assert!(view.undo_stack.is_empty());
                assert_eq!(view.redo_stack.len(), 1);
            });
            assert_eq!(
                fixture.messages.borrow().last().map(String::as_str),
                Some("Put “a.txt” back")
            );

            fixture.act(&mut app, FileTreeAction::Redo);
            fixture
                .wait_until(&mut app, "the item is trashed again", |view| {
                    view.undo_stack.len() == 1
                })
                .await;
            assert!(!a.exists());
            assert!(!fixture.is_shown(&app, &a));
            assert!(fixture.errors.borrow().is_empty());
        });
    });
}

#[test]
fn undo_reverses_a_move_and_a_paste_but_leaves_a_changed_copy() {
    VirtualFS::test("file_tree_undo_move_paste", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        let b = tree.join("b.txt");
        let pkg = tree.join("pkg");
        write(&a, "a");
        write(&b, "b");
        write(&pkg.join("inner.txt"), "inner");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            // A move: undone as one step, and the editors are told the file is back.
            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::CutSelection);
            fixture.select(&mut app, &pkg);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &pkg.join("a.txt"))
                .await;
            fixture.act(&mut app, FileTreeAction::Undo);
            fixture.wait_until_revealed(&mut app, &a).await;
            assert_eq!(read(&a), "a");
            assert!(!pkg.join("a.txt").exists());
            assert_eq!(
                *fixture.reported.borrow(),
                [
                    Reported::Renamed(a.clone(), pkg.join("a.txt")),
                    Reported::Renamed(pkg.join("a.txt"), a.clone()),
                ]
            );

            // A paste: the copy goes to the trash.
            fixture.select(&mut app, &b);
            fixture.act(&mut app, FileTreeAction::CopySelection);
            fixture.select(&mut app, &pkg);
            fixture.act(&mut app, FileTreeAction::Paste);
            fixture
                .wait_until_revealed(&mut app, &pkg.join("b.txt"))
                .await;
            fixture.act(&mut app, FileTreeAction::Undo);
            fixture
                .wait_until(&mut app, "the copy is trashed", |view| {
                    !view.is_reversing && view.redo_stack.len() == 1
                })
                .await;
            assert!(!pkg.join("b.txt").exists());
            assert_eq!(read(&pkg.join(TEST_TRASH_DIR).join("b.txt")), "b");
            assert_eq!(read(&b), "b");

            // A copy that was written to is left alone, and the step stays undoable.
            fixture.act(&mut app, FileTreeAction::Redo);
            fixture
                .wait_until(&mut app, "the copy is made again", |view| {
                    !view.is_reversing && view.undo_stack.len() == 1
                })
                .await;
            write(&pkg.join("b.txt"), "edited since");
            fixture.act(&mut app, FileTreeAction::Undo);
            fixture
                .wait_until(&mut app, "the refusal is reported", |_| {
                    !fixture.errors.borrow().is_empty()
                })
                .await;
            assert_eq!(
                *fixture.errors.borrow(),
                [
                    "Couldn’t undo: “b.txt” has changed since it was created, so it was left in place"
                ]
            );
            assert_eq!(read(&pkg.join("b.txt")), "edited since");
            fixture
                .view
                .read(&app, |view, _ctx| assert_eq!(view.undo_stack.len(), 1));
        });
    });
}

#[test]
fn undo_reverses_a_rename() {
    VirtualFS::test("file_tree_undo_rename", |dirs, mut vfs| {
        vfs.mkdir("tree");
        let tree = dirs.tests().join("tree");
        let a = tree.join("a.txt");
        write(&a, "a");

        App::test((), |mut app| async move {
            let fixture = open_tree(&mut app, &tree).await;

            fixture.select(&mut app, &a);
            fixture.act(&mut app, FileTreeAction::RenameSelected);
            fixture.confirm_name(&mut app, "z.txt");
            assert_eq!(read(&tree.join("z.txt")), "a");
            assert_eq!(*fixture.messages.borrow(), ["Renamed “a.txt” to “z.txt”"]);

            fixture.act(&mut app, FileTreeAction::Undo);
            fixture.wait_until_revealed(&mut app, &a).await;
            assert_eq!(read(&a), "a");
            assert!(!tree.join("z.txt").exists());
        });
    });
}
