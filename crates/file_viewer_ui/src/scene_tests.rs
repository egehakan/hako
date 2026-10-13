//! Tests that lay out, paint and drive a viewer in a window of the test platform.

use std::cell::RefCell;
use std::io::{Cursor, Write};
use std::rc::Rc;

use futures::executor::block_on;
use image::{DynamicImage, ImageFormat, Rgba};
use warpui::event::ModifiersState;
use warpui::platform::WindowStyle;
use warpui::{App, EntityIdSet, Event, Presenter, ViewHandle, WindowId, WindowInvalidation};

use super::*;

const WINDOW: (f32, f32) = (800., 600.);
const DEVICE_SCALE: f32 = 2.;

/// What one paint of the window drew. Text is left out: the test platform lays none out.
struct Painted {
    images: usize,
    rects: usize,
}

struct Harness {
    window_id: WindowId,
    view: ViewHandle<FileViewerView>,
    presenter: Rc<RefCell<Presenter>>,
}

impl Harness {
    /// Opens a window on a viewer of `kind` for `path`.
    fn open(app: &mut App, path: PathBuf, kind: ViewerKind) -> Self {
        app.add_singleton_model(|_| Appearance::mock());
        app.update(init);
        let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            FileViewerView::new(path, kind, None, ctx)
        });
        Self {
            window_id,
            view,
            presenter: Rc::new(RefCell::new(Presenter::new(window_id))),
        }
    }

    fn paint(&self, app: &mut App) -> Painted {
        let mut updated = EntityIdSet::default();
        updated.insert(app.root_view_id(self.window_id).unwrap());
        let invalidation = WindowInvalidation {
            updated,
            ..Default::default()
        };
        app.update(|ctx| {
            let mut presenter = self.presenter.borrow_mut();
            presenter.invalidate(invalidation, ctx);
            let scene = presenter.build_scene(vec2f(WINDOW.0, WINDOW.1), DEVICE_SCALE, None, ctx);
            Painted {
                images: scene.layers().map(|layer| layer.images.len()).sum(),
                rects: scene.layers().map(|layer| layer.rects.len()).sum(),
            }
        })
    }

    fn send(&self, app: &mut App, event: Event) -> bool {
        app.update(|ctx| ctx.simulate_window_event(event, self.window_id, self.presenter.clone()))
    }

    fn wheel(&self, app: &mut App, delta: Vector2F, zoom: bool) -> bool {
        self.send(
            app,
            Event::ScrollWheel {
                position: vec2f(400., 400.),
                delta,
                precise: true,
                modifiers: ModifiersState {
                    cmd: zoom,
                    ..Default::default()
                },
            },
        )
    }

    /// Puts content in the viewer as a finished load would. The load the constructor started
    /// is superseded first, so that its result cannot replace the content mid-test.
    fn show(
        &self,
        app: &mut App,
        show: impl FnOnce(&mut FileViewerView, &mut ViewContext<FileViewerView>),
    ) {
        self.view.update(app, |view, ctx| {
            view.generation += 1;
            show(view, ctx);
        });
    }

    fn read<T>(&self, app: &App, read: impl FnOnce(&FileViewerView) -> T) -> T {
        app.read(|ctx| read(self.view.as_ref(ctx)))
    }
}

fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
    let image = RgbaImage::from_pixel(width, height, Rgba([200, 40, 40, alpha]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

/// Writes a PDF of blank pages, each `width` by `height` points.
fn write_pdf(path: &Path, pages: usize, width: u32, height: u32) {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            (0..pages)
                .map(|page| format!("{} 0 R", page + 3))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ];
    objects.extend(
        (0..pages)
            .map(|_| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] >>")),
    );

    let mut file = Vec::new();
    file.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(file.len());
        write!(file, "{} 0 obj\n{object}\nendobj\n", index + 1).unwrap();
    }
    let xref = file.len();
    write!(file, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets {
        write!(file, "{offset:010} 00000 n \n").unwrap();
    }
    write!(
        file,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    fs::write(path, file).unwrap();
}

#[test]
fn a_binary_file_shows_a_message_and_the_header() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.bin");
        fs::write(&path, [0u8; 2048]).unwrap();
        let harness = Harness::open(&mut app, path, ViewerKind::Binary);

        let painted = harness.paint(&mut app);

        assert_eq!(painted.images, 0);
        assert!(painted.rects >= 1);
        let info = harness.read(&app, |view| view.info_text());
        assert_eq!(info, "Binary file  ·  2.0 KB");
        // Painting told the viewer the size it has: the window less the header.
        let viewport = harness.read(&app, |view| view.canvas.viewport()).unwrap();
        assert!(viewport.size.width >= WINDOW.0);
        assert!(viewport.size.height > 300.);
        assert!(viewport.size.height < viewport.size.width);
        assert_eq!(viewport.device_scale, DEVICE_SCALE);
    });
}

#[test]
fn an_image_is_fitted_then_zoomed_and_panned_with_the_wheel() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        fs::write(&path, png(1600, 1200, 255)).unwrap();
        let harness = Harness::open(&mut app, path, ViewerKind::Image);
        // Loading is still under way: the busy indicator is up.
        assert!(harness.paint(&mut app).rects >= BUSY_DOTS);

        let decoded = image_decoding::decode_bytes(&png(1600, 1200, 255)).unwrap();
        harness.show(&mut app, |view, ctx| view.show_image(decoded, ctx));
        let painted = harness.paint(&mut app);
        assert_eq!(painted.images, 1);
        let viewport = harness.read(&app, |view| view.viewport().size);
        let content = Size::new(1600., 1200.);
        let fitted = harness.read(&app, |view| view.transform.scale(content, viewport));
        assert!(fitted < 0.5);
        assert!(
            harness
                .read(&app, |view| view.info_text())
                .starts_with("PNG  ·  1600 × 1200")
        );

        // Scrolling up with the command key held zooms in around the pointer.
        assert!(harness.wheel(&mut app, vec2f(0., 60.), true));
        let zoomed = harness.read(&app, |view| view.transform.scale(content, viewport));
        assert!(zoomed > fitted);
        assert!(matches!(
            harness.read(&app, |view| view.transform.mode),
            ZoomMode::Manual(_)
        ));

        // At natural size the image no longer fits, and the wheel pans it.
        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::ActualSize, ctx);
        });
        harness.paint(&mut app);
        assert!(harness.wheel(&mut app, vec2f(-30., -50.), false));
        let scroll = harness.read(&app, |view| view.transform.scroll);
        assert_eq!(scroll, Offset::new(30., 50.));
        assert_eq!(harness.paint(&mut app).images, 1);

        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::FitToWindow, ctx);
        });
        assert_eq!(
            harness.read(&app, |view| view.transform.mode),
            ZoomMode::Fit
        );
    });
}

#[test]
fn a_transparent_image_is_drawn_over_a_backdrop() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        fs::write(&path, png(64, 64, 128)).unwrap();
        let harness = Harness::open(&mut app, path, ViewerKind::Image);
        let decoded = image_decoding::decode_bytes(&png(64, 64, 128)).unwrap();
        harness.show(&mut app, |view, ctx| view.show_image(decoded, ctx));

        let painted = harness.paint(&mut app);

        // One backdrop tile under the image, then the image.
        assert_eq!(painted.images, 2);
    });
}

#[test]
fn dragging_pans_and_a_press_takes_the_focus() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        fs::write(&path, png(64, 64, 255)).unwrap();
        let harness = Harness::open(&mut app, path, ViewerKind::Image);
        let decoded = image_decoding::decode_bytes(&png(2400, 2400, 255)).unwrap();
        harness.show(&mut app, |view, ctx| {
            view.show_image(decoded, ctx);
            view.handle_action(&FileViewerAction::ActualSize, ctx);
        });
        harness.paint(&mut app);

        let press = Event::LeftMouseDown {
            position: vec2f(400., 400.),
            modifiers: Default::default(),
            click_count: 1,
            is_first_mouse: false,
        };
        assert!(harness.send(&mut app, press));
        let drag = Event::LeftMouseDragged {
            position: vec2f(380., 360.),
            modifiers: Default::default(),
        };
        assert!(harness.send(&mut app, drag));

        let scroll = harness.read(&app, |view| view.transform.scroll);
        assert_eq!(scroll, Offset::new(20., 40.));
    });
}

#[test]
fn a_table_draws_its_header_and_the_rows_in_view() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        let mut text = String::from("name,amount\n");
        for row in 0..500 {
            text.push_str(&format!("row {row},{}\n", row * 3));
        }
        fs::write(&path, &text).unwrap();
        let harness = Harness::open(&mut app, path, ViewerKind::Table);
        let table = Arc::new(Table::parse(Arc::from(text.as_str()), None));
        harness.show(&mut app, |view, ctx| view.show_table(table, ctx));

        // The background of the canvas and of the header row, at least.
        let painted = harness.paint(&mut app);
        assert!(painted.rects >= 2);
        assert_eq!(
            harness.read(&app, |view| view.info_text()),
            format!(
                "500 rows × 2 columns  ·  comma-separated  ·  {}",
                format_bytes(text.len() as u64)
            )
        );

        // Scrolling moves by whole rows.
        assert!(harness.wheel(&mut app, vec2f(0., -(TABLE_ROW_HEIGHT * 3. + 2.)), false));
        let top_row = harness.read(&app, |view| match &view.body {
            Body::Table(table) => table.scroll.top_row,
            Body::Loading(_) | Body::Message(_) | Body::Peek(_) | Body::Image(_) | Body::Pdf(_) => {
                usize::MAX
            }
        });
        assert_eq!(top_row, 3);
        assert!(harness.paint(&mut app).rects >= 2);
    });
}

#[cfg(target_os = "macos")]
#[test]
fn a_document_shows_its_pages_and_moves_between_them() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paper.pdf");
        write_pdf(&path, 3, 400, 500);
        let harness = Harness::open(&mut app, path.clone(), ViewerKind::Pdf);
        let opened = block_on(PdfWorker::open(path, None)).unwrap();
        assert_eq!(opened.pages.len(), 3);
        harness.show(&mut app, |view, ctx| view.show_pdf(opened, ctx));

        // The paper of the pages in view is drawn before any raster arrives.
        let painted = harness.paint(&mut app);
        assert!(painted.rects >= 2);
        assert_eq!(harness.read(&app, |view| view.current_page()), Some((0, 3)));
        assert!(
            harness
                .read(&app, |view| view.info_text())
                .contains("Page 1 of 3")
        );

        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::NextPage, ctx);
        });
        assert_eq!(harness.read(&app, |view| view.current_page()), Some((1, 3)));

        // Typing a page number and confirming goes to that page.
        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::PageDigit(3), ctx);
        });
        assert!(
            harness
                .read(&app, |view| view.info_text())
                .contains("Go to page 3")
        );
        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::GoToTypedPage, ctx);
        });
        assert_eq!(harness.read(&app, |view| view.current_page()), Some((2, 3)));
        harness.paint(&mut app);

        // Strips for the pages in view were asked for at the scale of the display.
        let pending = harness.read(&app, |view| match &view.body {
            Body::Pdf(pdf) => pdf.pending.len() + pdf.strips.len(),
            Body::Loading(_)
            | Body::Message(_)
            | Body::Peek(_)
            | Body::Image(_)
            | Body::Table(_) => 0,
        });
        assert!(pending > 0);
    });
}

#[test]
fn a_large_text_file_shows_its_start_and_offers_the_editor() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let lines = warp_util::large_file::AUTO_OPEN_MAX_LINES + 50;
        let mut text = String::new();
        for index in 0..lines {
            text.push_str(&format!("entry {index}\n"));
        }
        std::fs::write(&path, &text).unwrap();
        let harness = Harness::open(&mut app, path.clone(), ViewerKind::LargeText);

        // Nothing is offered before the file has been looked at.
        assert_eq!(harness.read(&app, |view| view.open_in_editor_label()), None);
        let large_text = LargeText::read(&path).unwrap();
        harness.show(&mut app, |view, ctx| view.show_peek(large_text, ctx));

        assert!(harness.paint(&mut app).rects >= 1);
        assert_eq!(
            harness.read(&app, |view| view.info_text()),
            format!(
                "Large file  ·  150,050 lines  ·  {}",
                format_bytes(text.len() as u64)
            )
        );
        assert_eq!(
            harness.read(&app, |view| view.open_in_editor_label()),
            Some("Open in editor anyway")
        );
        let (rows, notice) = harness.read(&app, |view| match &view.body {
            Body::Peek(peek) => (peek.rows.clone(), peek.notice.clone()),
            Body::Loading(_)
            | Body::Message(_)
            | Body::Image(_)
            | Body::Pdf(_)
            | Body::Table(_) => panic!("The start of the file is not shown"),
        });
        assert_eq!(rows.len(), warp_util::large_file::PEEK_MAX_LINES + 1);
        assert_eq!(rows[0], "entry 0");
        assert!(
            notice.starts_with("This file has 150,050 lines"),
            "{notice}"
        );

        // Scrolling moves by whole lines and stops at the last one.
        assert!(harness.wheel(&mut app, vec2f(0., -(PEEK_ROW_HEIGHT * 5. + 1.)), false));
        let top_row = |app: &App| {
            harness.read(app, |view| match &view.body {
                Body::Peek(peek) => peek.scroll.top_row,
                Body::Loading(_)
                | Body::Message(_)
                | Body::Image(_)
                | Body::Pdf(_)
                | Body::Table(_) => usize::MAX,
            })
        };
        assert_eq!(top_row(&app), 5);
        assert!(harness.wheel(&mut app, vec2f(0., -1_000_000.), false));
        assert!(top_row(&app) < rows.len());
        harness.paint(&mut app);

        // Asking for the editor is passed on to whoever holds the tab.
        let asked = Rc::new(RefCell::new(0));
        let counter = asked.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&harness.view, move |_, event, _| match event {
                FileViewerEvent::OpenInEditorRequested => *counter.borrow_mut() += 1,
                FileViewerEvent::Focused => {}
            });
        });
        harness.view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewerAction::OpenInEditor, ctx);
        });
        assert_eq!(*asked.borrow(), 1);
    });
}

#[test]
fn a_file_that_was_not_reopened_is_not_read_and_offers_to_open() {
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        // Not even there: a viewer of this kind does not look at the file.
        let path = dir.path().join("gone.sql");
        let harness = Harness::open(&mut app, path, ViewerKind::NotLoaded);

        let message = harness.read(&app, |view| match &view.body {
            Body::Message(message) => message.clone(),
            Body::Loading(_) | Body::Peek(_) | Body::Image(_) | Body::Pdf(_) | Body::Table(_) => {
                String::new()
            }
        });
        assert!(
            message.contains("closed unexpectedly while loading it"),
            "{message}"
        );
        assert_eq!(
            harness.read(&app, |view| view.open_in_editor_label()),
            Some("Open")
        );
        assert_eq!(harness.read(&app, |view| view.info_text()), "Not loaded");
        harness.paint(&mut app);
    });
}
