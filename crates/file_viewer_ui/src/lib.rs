//! Viewers for the files an editor tab cannot show as text, or shows better rendered: images,
//! PDF and office documents, tables, and anything else binary.
//!
//! One view hosts every kind. Decoding, layout and conversion live in the `file_viewers` crate;
//! this one loads the content off the main thread, draws it and routes input.

mod canvas;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use file_viewers::cache::ByteBudgetCache;
use file_viewers::csv::{self, GridScroll, Table};
use file_viewers::geometry::{self, Offset, Rect, Size, ViewTransform, ZoomMode};
use file_viewers::image::{self as image_decoding, DecodeError, DecodedImage, Pyramid};
use file_viewers::kind::ViewerKind;
use file_viewers::large_text::{LargeText, lines_text};
use file_viewers::office::{self, ConvertError};
use file_viewers::pdf::{OpenedPdf, PageLayout, PdfError, PdfWorker, Strip, StripKey};
use file_viewers::scene::{self, Placement, Viewport};
use file_viewers::svg::{self, SvgRaster};
use file_viewers::{format_bytes, quick_look, run_blocking};
use image::RgbaImage;
use instant::Instant;
use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors as blended_colors;
use warpui::r#async::Timer;
use warpui::elements::{
    Align, Clipped, ConstrainedBox, Container, CrossAxisAlignment, Expanded, Flex, MainAxisSize,
    MouseStateHandle, ParentElement, Shrinkable, Text, Wrap,
};
use warpui::image_cache::StaticImage;
use warpui::keymap::{self, EditableBinding, FixedBinding};
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, id,
};

use self::canvas::{BackdropTiles, CanvasHandle, Frame, Shape, ViewerCanvas};

/// Memory the rasters of a PDF document may take.
const STRIP_CACHE_BYTES: usize = 256 << 20;

/// How far beyond the view pages are rasterised ahead, in viewport heights.
const STRIP_OVERSCAN: f32 = 0.75;

/// How long the zoom has to rest before content is rasterised again for the new scale.
const RASTER_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long after its content arrives a viewer waits before reading the size it was drawn at.
const FIRST_PAINT_DELAY: Duration = Duration::from_millis(80);

/// Scale an SVG is first rasterised at, in pixels per CSS pixel.
const INITIAL_SVG_SCALE: f32 = 2.;

/// Side of one tile of the pattern behind transparent images, in logical pixels.
const BACKDROP_TILE: f32 = 256.;
const BACKDROP_CELL_PIXELS: u32 = 16;
const BACKDROP_CELLS: u32 = 32;

/// The busy indicator: a row of dots lit one after the other.
const BUSY_DOTS: usize = 3;
const BUSY_DOT_SIZE: f32 = 6.;
const BUSY_DOT_GAP: f32 = 8.;
const BUSY_DOTS_BELOW_CENTRE: f32 = 28.;
const BUSY_STEP: Duration = Duration::from_millis(280);

const HEADER_ROW_HEIGHT: f32 = 28.;
const PEEK_ROW_HEIGHT: f32 = 20.;

/// Said at the end of a peeked line that goes on in the file.
const LINE_TRUNCATED_MARKER: &str = "  … line truncated";
const TABLE_ROW_HEIGHT: f32 = 24.;
const TABLE_CELL_PADDING: f32 = 8.;
const TABLE_MIN_COLUMN_WIDTH: f32 = 60.;
const TABLE_MAX_COLUMN_WIDTH: f32 = 420.;

/// Characters of a cell that are laid out; the rest cannot be seen in a column of the maximum
/// width anyway.
const TABLE_MAX_CELL_CHARS: usize = 160;

/// Width of a character of the monospace font relative to its size.
const MONOSPACE_ADVANCE: f32 = 0.62;

/// Keymap context a viewer adds while it shows something that zooms, for bindings that should
/// not take a key away from the rest of the app otherwise.
pub const ZOOMABLE_CONTEXT: &str = "FileViewer_Zoomable";

pub fn init(app: &mut AppContext) {
    let (editable, fixed) = bindings();
    app.register_editable_bindings(editable);
    app.register_fixed_bindings(fixed);
}

fn bindings() -> (Vec<EditableBinding>, Vec<FixedBinding>) {
    let viewer = id!(FileViewerView::ui_name());
    let editable = vec![
        EditableBinding::new(
            "file_viewer:zoom_in",
            "Zoom in on the viewed file",
            FileViewerAction::ZoomIn,
        )
        .with_context_predicate(viewer.clone())
        .with_key_binding("cmdorctrl-="),
        EditableBinding::new(
            "file_viewer:zoom_out",
            "Zoom out of the viewed file",
            FileViewerAction::ZoomOut,
        )
        .with_context_predicate(viewer.clone())
        .with_key_binding("cmdorctrl--"),
        EditableBinding::new(
            "file_viewer:fit_to_window",
            "Fit the viewed file to the window",
            FileViewerAction::FitToWindow,
        )
        .with_context_predicate(viewer.clone())
        .with_key_binding("cmdorctrl-0"),
        EditableBinding::new(
            "file_viewer:next_page",
            "Go to the next page of the viewed document",
            FileViewerAction::NextPage,
        )
        .with_context_predicate(viewer.clone())
        .with_key_binding("pagedown"),
        EditableBinding::new(
            "file_viewer:previous_page",
            "Go to the previous page of the viewed document",
            FileViewerAction::PreviousPage,
        )
        .with_context_predicate(viewer.clone())
        .with_key_binding("pageup"),
    ];

    // Typing a page number and pressing enter goes to that page.
    let mut page_entry: Vec<FixedBinding> = (0..=9u8)
        .map(|digit| {
            FixedBinding::new(
                digit.to_string(),
                FileViewerAction::PageDigit(digit),
                viewer.clone(),
            )
        })
        .collect();
    page_entry.push(FixedBinding::new(
        "enter",
        FileViewerAction::GoToTypedPage,
        viewer.clone(),
    ));
    page_entry.push(FixedBinding::new(
        "escape",
        FileViewerAction::CancelPageEntry,
        viewer,
    ));
    (editable, page_entry)
}

#[derive(Clone, Debug)]
pub enum FileViewerAction {
    /// Asks for the file in an editor, whatever stood in the way of that so far.
    OpenInEditor,
    OpenInDefaultApp,
    RevealInFileManager,
    QuickLook,
    ZoomIn,
    ZoomOut,
    ActualSize,
    FitToWindow,
    PreviousPage,
    NextPage,
    /// A digit typed towards the number of the page to go to.
    PageDigit(u8),
    GoToTypedPage,
    CancelPageEntry,
    /// The wheel moved over the content, at `position` relative to its top-left corner.
    Wheel {
        position: Vector2F,
        delta: Vector2F,
        precise: bool,
        zoom: bool,
    },
    Pressed {
        position: Vector2F,
        click_count: u32,
    },
    /// The mouse moved by `-delta` with the button held down on the content.
    Dragged {
        delta: Vector2F,
    },
    /// The content was drawn at a size or display scale the view has not seen yet.
    ViewportChanged,
}

#[derive(Clone, Debug)]
pub enum FileViewerEvent {
    /// The viewer was clicked and took the keyboard focus.
    Focused,
    /// The file should be shown in an editor in place of this viewer.
    OpenInEditorRequested,
}

/// Mouse state of the buttons in the header, kept across frames.
#[derive(Default)]
struct HeaderButtons {
    open_in_editor: MouseStateHandle,
    open: MouseStateHandle,
    reveal: MouseStateHandle,
    quick_look: MouseStateHandle,
    zoom_in: MouseStateHandle,
    zoom_out: MouseStateHandle,
    actual_size: MouseStateHandle,
    fit: MouseStateHandle,
    previous_page: MouseStateHandle,
    next_page: MouseStateHandle,
}

type GpuPyramid = Pyramid<Arc<StaticImage>>;

struct ImageFrame {
    pyramid: GpuPyramid,
    delay_ms: u32,
}

/// The text an SVG is drawn from, kept to draw it again sharper when zoomed in.
struct SvgSource {
    text: Arc<str>,
    pixels_per_unit: f32,
    rendering: bool,
}

struct ImageBody {
    /// Size of the image in its own units: pixels, or CSS pixels for an SVG.
    content: Size,
    format: String,
    frames: Arc<Vec<ImageFrame>>,
    has_transparency: bool,
    /// When the animation started.
    started: Instant,
    svg: Option<SvgSource>,
}

struct PdfBody {
    worker: PdfWorker,
    layout: Arc<PageLayout>,
    strips: ByteBudgetCache<StripKey, (Strip, Arc<StaticImage>)>,
    /// Strips being rasterised, with the flag that cancels each.
    pending: HashMap<StripKey, Arc<AtomicBool>>,
    /// Digits typed so far towards a page number.
    page_entry: String,
}

struct TableBody {
    table: Arc<Table>,
    widths: Vec<f32>,
    scroll: GridScroll,
}

/// The start of a text file that is too large for an editor.
struct PeekBody {
    text: LargeText,
    notice: String,
    /// The lines as drawn, each with its marker if it goes on in the file.
    rows: Vec<String>,
    scroll: GridScroll,
}

enum Body {
    Loading(&'static str),
    /// Nothing to show but an explanation.
    Message(String),
    Peek(PeekBody),
    Image(ImageBody),
    Pdf(PdfBody),
    Table(TableBody),
}

/// What a background load produced.
enum Loaded {
    Image(Result<DecodedImage, DecodeError>),
    Svg(Result<(Arc<str>, SvgRaster), String>),
    Table(Result<Arc<Table>, String>),
    Converted(Result<PathBuf, ConvertError>),
    LargeText(Result<LargeText, String>),
}

pub struct FileViewerView {
    path: PathBuf,
    kind: ViewerKind,
    file_size: Option<u64>,
    /// When the file was last written as of the load that produced `body`.
    loaded_modified: Option<SystemTime>,
    /// When the current load started, which is what the busy indicator counts from.
    loading_since: Instant,
    body: Body,
    transform: ViewTransform,
    canvas: CanvasHandle,
    buttons: HeaderButtons,
    /// What was done differently from showing the file as it is.
    notes: Vec<String>,
    /// Counts loads, so that the result of one that was superseded is dropped.
    generation: u64,
    /// Counts zoom changes, so that only the last of a burst rasterises again.
    raster_request: u64,
    backdrop: Arc<StaticImage>,
}

impl FileViewerView {
    /// Shows the file at `path` with the viewer for `kind`. `text` is the content to show in
    /// place of what is on disk, for kinds that are rendered from text.
    pub fn new(
        path: PathBuf,
        kind: ViewerKind,
        text: Option<Arc<str>>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let backdrop = image_decoding::checkerboard(
            BACKDROP_CELL_PIXELS,
            BACKDROP_CELLS,
            [255, 255, 255, 255],
            [204, 204, 204, 255],
        );
        let mut view = Self {
            path,
            kind,
            file_size: None,
            loaded_modified: None,
            loading_since: Instant::now(),
            body: Body::Loading("Loading…"),
            transform: ViewTransform::new(default_zoom_mode(kind)),
            canvas: CanvasHandle::default(),
            buttons: HeaderButtons::default(),
            notes: Vec::new(),
            generation: 0,
            raster_request: 0,
            backdrop: Arc::new(StaticImage::from_rgba(backdrop)),
        };
        view.load(text, ctx);
        view
    }

    pub fn kind(&self) -> ViewerKind {
        self.kind
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
        self.reload_if_changed(ctx);
    }

    /// Loads the file again if it was written since it was loaded. Coming back to the viewer
    /// is when a stale picture would be noticed, so that is when this is checked. Kinds
    /// rendered from text follow their editor's buffer instead.
    fn reload_if_changed(&mut self, ctx: &mut ViewContext<Self>) {
        if self.kind.is_text_backed() {
            return;
        }
        let Ok(metadata) = fs::metadata(&self.path) else {
            return;
        };
        let changed = Some(metadata.len()) != self.file_size
            || metadata.modified().ok() != self.loaded_modified;
        if changed {
            self.load(None, ctx);
        }
    }

    /// Points the viewer at the file's new location after a rename.
    pub fn set_path(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        self.path = path;
        ctx.notify();
    }

    /// Shows `text` in place of the content shown so far, for kinds that are rendered from
    /// text.
    pub fn set_text(&mut self, text: Arc<str>, ctx: &mut ViewContext<Self>) {
        if self.kind.is_text_backed() {
            self.load(Some(text), ctx);
        }
    }

    fn viewport(&self) -> Viewport {
        self.canvas.viewport().unwrap_or(Viewport::ASSUMED)
    }

    fn load(&mut self, text: Option<Arc<str>>, ctx: &mut ViewContext<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let metadata = fs::metadata(&self.path).ok();
        self.file_size = metadata.as_ref().map(|metadata| metadata.len());
        self.loaded_modified = metadata.and_then(|metadata| metadata.modified().ok());
        self.loading_since = Instant::now();
        self.notes.clear();
        let path = self.path.clone();

        match self.kind {
            ViewerKind::Binary => {
                self.body = Body::Message(
                    "This file is not text and has no viewer here. Open it in its default app \
                     or look at it with Quick Look."
                        .to_string(),
                );
            }
            ViewerKind::NotLoaded => {
                self.body = Body::Message(
                    "This file was not reopened because the app closed unexpectedly while \
                     loading it."
                        .to_string(),
                );
            }
            ViewerKind::LargeText => {
                self.body = Body::Loading("Reading the start of the file…");
                self.spawn_load(generation, ctx, move || {
                    Loaded::LargeText(LargeText::read(&path).map_err(|err| {
                        log::warn!("Could not read the start of {}: {err}", path.display());
                        format!("The file could not be read: {err}")
                    }))
                });
            }
            ViewerKind::Image => {
                self.body = Body::Loading("Loading…");
                self.spawn_load(generation, ctx, move || {
                    Loaded::Image(image_decoding::decode_file(&path))
                });
            }
            ViewerKind::Svg => {
                if !matches!(self.body, Body::Image(_)) {
                    self.body = Body::Loading("Loading…");
                }
                self.spawn_load(generation, ctx, move || {
                    Loaded::Svg(render_svg(&path, text, INITIAL_SVG_SCALE))
                });
            }
            ViewerKind::Table => {
                if !matches!(self.body, Body::Table(_)) {
                    self.body = Body::Loading("Loading…");
                }
                self.spawn_load(generation, ctx, move || {
                    Loaded::Table(read_table(&path, text))
                });
            }
            ViewerKind::Pdf => {
                self.body = Body::Loading("Loading…");
                self.open_pdf(path, generation, ctx);
            }
            ViewerKind::Office => {
                let Some(converter) = office::system_converter() else {
                    self.body = Body::Message(ConvertError::Unsupported.to_string());
                    return;
                };
                self.body = Body::Loading("Converting the document for viewing…");
                let cache_dir = warp_core::paths::cache_dir();
                self.spawn_load(generation, ctx, move || {
                    Loaded::Converted(office::convert_cached(
                        converter.as_ref(),
                        &path,
                        &cache_dir,
                        office::DEFAULT_TIMEOUT,
                    ))
                });
            }
        }
        ctx.notify();
    }

    /// Runs `work` off the main thread and hands its result to [`Self::loaded`], unless
    /// another load started in the meantime.
    fn spawn_load(
        &mut self,
        generation: u64,
        ctx: &mut ViewContext<Self>,
        work: impl FnOnce() -> Loaded + Send + 'static,
    ) {
        ctx.spawn(run_blocking(work), move |me, loaded, ctx| {
            if me.generation != generation {
                return;
            }
            match loaded {
                Some(loaded) => me.loaded(loaded, ctx),
                None => me.fail("The file could not be loaded.".to_string(), ctx),
            }
        });
    }

    fn fail(&mut self, message: String, ctx: &mut ViewContext<Self>) {
        self.body = Body::Message(message);
        ctx.notify();
    }

    fn loaded(&mut self, loaded: Loaded, ctx: &mut ViewContext<Self>) {
        match loaded {
            Loaded::Image(Ok(decoded)) => self.show_image(decoded, ctx),
            Loaded::Image(Err(err)) => self.fail(err.to_string(), ctx),
            Loaded::Svg(Ok((text, raster))) => self.show_svg(text, raster, ctx),
            Loaded::Svg(Err(message))
            | Loaded::Table(Err(message))
            | Loaded::LargeText(Err(message)) => self.fail(message, ctx),
            Loaded::LargeText(Ok(text)) => self.show_peek(text, ctx),
            Loaded::Table(Ok(table)) => self.show_table(table, ctx),
            Loaded::Converted(Ok(pdf)) => {
                self.notes
                    .push("Converted to PDF for viewing; the file itself is unchanged.".into());
                self.open_pdf(pdf, self.generation, ctx);
            }
            Loaded::Converted(Err(err)) => self.fail(err.to_string(), ctx),
        }
    }

    fn show_image(&mut self, decoded: DecodedImage, ctx: &mut ViewContext<Self>) {
        let content = decoded.content_size();
        self.notes = decoded.notes;
        let frames = decoded
            .frames
            .into_iter()
            .map(|frame| ImageFrame {
                pyramid: frame.pyramid.map(to_texture),
                delay_ms: frame.delay_ms,
            })
            .collect();
        self.body = Body::Image(ImageBody {
            content,
            format: decoded.format,
            frames: Arc::new(frames),
            has_transparency: decoded.has_transparency,
            started: Instant::now(),
            svg: None,
        });
        ctx.notify();
    }

    fn show_svg(&mut self, text: Arc<str>, raster: SvgRaster, ctx: &mut ViewContext<Self>) {
        let pixels_per_unit = raster.pixels_per_unit;
        let pyramid = Pyramid::build(raster.image, raster.size, true).map(to_texture);
        self.body = Body::Image(ImageBody {
            content: raster.size,
            format: "SVG".to_string(),
            frames: Arc::new(vec![ImageFrame {
                pyramid,
                delay_ms: 0,
            }]),
            has_transparency: true,
            started: Instant::now(),
            svg: Some(SvgSource {
                text,
                pixels_per_unit,
                rendering: false,
            }),
        });
        // The first raster was made before the scale on screen was known.
        self.schedule_raster(FIRST_PAINT_DELAY, ctx);
        ctx.notify();
    }

    fn show_peek(&mut self, text: LargeText, ctx: &mut ViewContext<Self>) {
        log::info!(
            "Showing the first {} lines of {} ({} bytes) {} ms after it was opened",
            text.peek.lines.len(),
            self.path.display(),
            text.bytes,
            self.loading_since.elapsed().as_millis()
        );
        self.file_size = Some(text.bytes);
        self.body = Body::Peek(PeekBody {
            notice: text.notice(),
            rows: peek_rows(&text),
            text,
            scroll: GridScroll::default(),
        });
        ctx.notify();
    }

    fn show_table(&mut self, table: Arc<Table>, ctx: &mut ViewContext<Self>) {
        if table.row_count() > csv::MAX_TABLE_ROWS {
            self.fail(
                format!(
                    "This table has {} rows, more than the {} shown as a table. Switch to the \
                     raw text to read it.",
                    table.row_count(),
                    csv::MAX_TABLE_ROWS
                ),
                ctx,
            );
            return;
        }
        let appearance = Appearance::as_ref(ctx);
        let char_width = appearance.monospace_font_size() * MONOSPACE_ADVANCE;
        let widths = csv::column_widths(
            &table.column_chars(),
            char_width,
            2. * TABLE_CELL_PADDING,
            TABLE_MIN_COLUMN_WIDTH,
            TABLE_MAX_COLUMN_WIDTH,
        );
        // Text edited while the table is up keeps the place it was scrolled to.
        let scroll = match &self.body {
            Body::Table(previous) => previous.scroll,
            Body::Loading(_) | Body::Message(_) | Body::Peek(_) | Body::Image(_) | Body::Pdf(_) => {
                GridScroll::default()
            }
        };
        self.body = Body::Table(TableBody {
            table,
            widths,
            scroll,
        });
        ctx.notify();
    }

    fn open_pdf(&mut self, path: PathBuf, generation: u64, ctx: &mut ViewContext<Self>) {
        ctx.spawn(PdfWorker::open(path, None), move |me, opened, ctx| {
            if me.generation != generation {
                return;
            }
            match opened {
                Ok(opened) => me.show_pdf(opened, ctx),
                Err(PdfError::PasswordRequired) => me.fail(
                    "This document is protected by a password, which cannot be entered here. \
                     Open it in its default app."
                        .to_string(),
                    ctx,
                ),
                Err(err) => me.fail(err.to_string(), ctx),
            }
        });
    }

    fn show_pdf(&mut self, opened: OpenedPdf, ctx: &mut ViewContext<Self>) {
        self.body = Body::Pdf(PdfBody {
            worker: opened.worker,
            layout: Arc::new(PageLayout::new(&opened.pages)),
            strips: ByteBudgetCache::new(STRIP_CACHE_BYTES),
            pending: HashMap::new(),
            page_entry: String::new(),
        });
        // Rasterise for the assumed viewport now, and again once the real one is known.
        self.request_strips(ctx);
        self.schedule_raster(FIRST_PAINT_DELAY, ctx);
        ctx.notify();
    }

    /// Size of the content in its own units, when the body has a zoomable extent.
    fn content_size(&self) -> Option<Size> {
        match &self.body {
            Body::Image(image) => Some(image.content),
            Body::Pdf(pdf) => Some(pdf.layout.content_size()),
            Body::Loading(_) | Body::Message(_) | Body::Peek(_) | Body::Table(_) => None,
        }
    }

    /// Rasterises for the current view after `delay`, unless the view changes again first.
    fn schedule_raster(&mut self, delay: Duration, ctx: &mut ViewContext<Self>) {
        self.raster_request += 1;
        let request = self.raster_request;
        ctx.spawn(Timer::after(delay), move |me, _, ctx| {
            if me.raster_request == request {
                me.request_strips(ctx);
                me.sharpen_svg(ctx);
            }
        });
    }

    /// Asks the worker for the strips the current view needs and does not have, and cancels
    /// the ones it no longer needs.
    fn request_strips(&mut self, ctx: &mut ViewContext<Self>) {
        let viewport = self.viewport();
        let generation = self.generation;
        let Body::Pdf(pdf) = &mut self.body else {
            return;
        };
        let wanted = scene::wanted_strips(&pdf.layout, &self.transform, viewport, STRIP_OVERSCAN);

        pdf.pending.retain(|key, cancelled| {
            let still_wanted = wanted.iter().any(|strip| strip.key == *key);
            if !still_wanted {
                cancelled.store(true, Ordering::Relaxed);
            }
            still_wanted
        });
        for strip in wanted {
            // Looking a strip up marks it as in use, which keeps it from being evicted.
            if pdf.strips.get(&strip.key).is_some() || pdf.pending.contains_key(&strip.key) {
                continue;
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            pdf.pending.insert(strip.key, cancelled.clone());
            ctx.spawn(
                pdf.worker.render(strip, cancelled),
                move |me, rendered, ctx| {
                    if me.generation == generation {
                        me.strip_rendered(strip, rendered, ctx);
                    }
                },
            );
        }
    }

    fn strip_rendered(
        &mut self,
        strip: Strip,
        rendered: Result<RgbaImage, PdfError>,
        ctx: &mut ViewContext<Self>,
    ) {
        let viewport = self.viewport();
        let Body::Pdf(pdf) = &mut self.body else {
            return;
        };
        pdf.pending.remove(&strip.key);
        match rendered {
            Ok(image) => {
                pdf.strips
                    .insert(strip.key, (strip, to_texture(image)), strip.byte_size());
            }
            Err(PdfError::Cancelled) => {}
            Err(err) => log::warn!("Could not rasterise a page of {:?}: {err}", self.path),
        }
        // Rasters made for an earlier zoom level stay up, stretched, until the ones for the
        // current level have all arrived.
        if pdf.pending.is_empty() {
            let layout = pdf.layout.clone();
            let transform = self.transform;
            pdf.strips
                .retain(|key| scene::is_at_wanted_scale(key, &layout, &transform, viewport));
        }
        ctx.notify();
    }

    /// Rasterises the SVG again if the view shows it larger than its current raster.
    fn sharpen_svg(&mut self, ctx: &mut ViewContext<Self>) {
        let viewport = self.viewport();
        let generation = self.generation;
        let Body::Image(image) = &mut self.body else {
            return;
        };
        let content = image.content;
        let Some(source) = image.svg.as_mut() else {
            return;
        };
        let on_screen = self.transform.scale(content, viewport.size) * viewport.device_scale;
        let wanted = svg::raster_scale_for(on_screen, content);
        if source.rendering || wanted <= source.pixels_per_unit {
            return;
        }
        source.rendering = true;
        let text = source.text.clone();
        ctx.spawn(
            run_blocking(move || svg::render(&text, wanted)),
            move |me, raster, ctx| {
                if me.generation != generation {
                    return;
                }
                let Body::Image(image) = &mut me.body else {
                    return;
                };
                let Some(source) = image.svg.as_mut() else {
                    return;
                };
                source.rendering = false;
                if let Some(Ok(raster)) = raster {
                    source.pixels_per_unit = raster.pixels_per_unit;
                    image.frames = Arc::new(vec![ImageFrame {
                        pyramid: Pyramid::build(raster.image, raster.size, true).map(to_texture),
                        delay_ms: 0,
                    }]);
                    ctx.notify();
                }
            },
        );
    }

    /// Called after anything that changes which part of the content is in view.
    fn view_changed(&mut self, zoomed: bool, ctx: &mut ViewContext<Self>) {
        if zoomed {
            self.schedule_raster(RASTER_DEBOUNCE, ctx);
        } else {
            self.request_strips(ctx);
        }
        ctx.notify();
    }

    fn zoom_to(&mut self, scale: f32, anchor: Offset, ctx: &mut ViewContext<Self>) {
        let Some(content) = self.content_size() else {
            return;
        };
        let viewport = self.viewport().size;
        self.transform.zoom_to(scale, anchor, content, viewport);
        self.view_changed(true, ctx);
    }

    fn step_zoom(&mut self, zoom_in: bool, ctx: &mut ViewContext<Self>) {
        let Some(content) = self.content_size() else {
            return;
        };
        let viewport = self.viewport().size;
        let scale = geometry::next_zoom_step(self.transform.scale(content, viewport), zoom_in);
        let centre = Offset::new(viewport.width / 2., viewport.height / 2.);
        self.zoom_to(scale, centre, ctx);
    }

    fn set_zoom_mode(&mut self, mode: ZoomMode, ctx: &mut ViewContext<Self>) {
        let Some(content) = self.content_size() else {
            return;
        };
        let viewport = self.viewport().size;
        self.transform.set_mode(mode, content, viewport);
        self.view_changed(true, ctx);
    }

    fn scroll_by(&mut self, delta: Offset, ctx: &mut ViewContext<Self>) {
        let viewport = self.viewport().size;
        match &mut self.body {
            Body::Table(table) => {
                let font_size = Appearance::as_ref(ctx).monospace_font_size();
                let number_width = row_number_width(table.table.row_count(), font_size);
                table.scroll.scroll_by(
                    delta.x,
                    delta.y,
                    TABLE_ROW_HEIGHT,
                    table.table.row_count(),
                    csv::visible_rows(viewport.height, TABLE_ROW_HEIGHT),
                    csv::max_scroll_x(&table.widths, viewport.width - number_width),
                );
                ctx.notify();
            }
            Body::Peek(peek) => {
                let font_size = Appearance::as_ref(ctx).monospace_font_size();
                let char_width = font_size * MONOSPACE_ADVANCE;
                let number_width = row_number_width(peek.rows.len(), font_size);
                let longest = peek.rows.iter().map(|row| row.chars().count()).max();
                let text_width = longest.unwrap_or_default() as f32 * char_width;
                peek.scroll.scroll_by(
                    delta.x,
                    delta.y,
                    PEEK_ROW_HEIGHT,
                    peek.rows.len(),
                    peek_visible_rows(viewport.height),
                    text_width - (viewport.width - number_width),
                );
                ctx.notify();
            }
            Body::Image(_) | Body::Pdf(_) => {
                let Some(content) = self.content_size() else {
                    return;
                };
                self.transform.scroll_by(delta, content, viewport);
                self.view_changed(false, ctx);
            }
            Body::Loading(_) | Body::Message(_) => {}
        }
    }

    /// The label of the button that asks for the file in an editor, when that can be asked for.
    fn open_in_editor_label(&self) -> Option<&'static str> {
        match (&self.kind, &self.body) {
            (ViewerKind::NotLoaded, _) => Some("Open"),
            (ViewerKind::LargeText, Body::Peek(peek)) => peek
                .text
                .can_open_in_editor()
                .then_some("Open in editor anyway"),
            (ViewerKind::LargeText, _) => None,
            (
                ViewerKind::Image
                | ViewerKind::Svg
                | ViewerKind::Pdf
                | ViewerKind::Table
                | ViewerKind::Office
                | ViewerKind::Binary,
                _,
            ) => None,
        }
    }

    /// Index of the page in view, counted from zero, and the number of pages.
    fn current_page(&self) -> Option<(usize, usize)> {
        let Body::Pdf(pdf) = &self.body else {
            return None;
        };
        let content = pdf.layout.content_size();
        let visible = self
            .transform
            .visible_content(content, self.viewport().size);
        Some((pdf.layout.current_page(visible), pdf.layout.page_count()))
    }

    fn go_to_page(&mut self, page: usize, ctx: &mut ViewContext<Self>) {
        let viewport = self.viewport().size;
        let Body::Pdf(pdf) = &self.body else {
            return;
        };
        let page = page.min(pdf.layout.page_count().saturating_sub(1));
        let Some(target) = pdf.layout.scroll_target(page) else {
            return;
        };
        self.transform
            .scroll_to_content_y(target, pdf.layout.content_size(), viewport);
        self.view_changed(false, ctx);
    }

    fn step_page(&mut self, forward: bool, ctx: &mut ViewContext<Self>) {
        let Some((page, _)) = self.current_page() else {
            return;
        };
        let target = if forward {
            page + 1
        } else {
            page.saturating_sub(1)
        };
        self.go_to_page(target, ctx);
    }

    fn info_text(&self) -> String {
        let mut parts = Vec::new();
        let viewport = self.viewport().size;
        match &self.body {
            Body::Image(image) => {
                parts.push(image.format.clone());
                parts.push(format!(
                    "{} × {}",
                    image.content.width.round(),
                    image.content.height.round()
                ));
                if image.frames.len() > 1 {
                    parts.push(format!("{} frames", image.frames.len()));
                }
                let scale = self.transform.scale(image.content, viewport);
                parts.push(geometry::format_zoom(scale));
            }
            Body::Pdf(pdf) => {
                parts.push(self.kind.label().to_string());
                if !pdf.page_entry.is_empty() {
                    parts.push(format!("Go to page {} ⏎", pdf.page_entry));
                } else if let Some((page, count)) = self.current_page() {
                    parts.push(format!("Page {} of {count}", page + 1));
                }
                let scale = self.transform.scale(pdf.layout.content_size(), viewport);
                parts.push(geometry::format_zoom(scale));
            }
            Body::Table(table) => {
                parts.push(format!(
                    "{} rows × {} columns",
                    table.table.row_count(),
                    table.table.column_count()
                ));
                parts.push(format!(
                    "{}-separated",
                    csv::delimiter_name(table.table.delimiter())
                ));
            }
            Body::Peek(peek) => {
                parts.push(self.kind.label().to_string());
                if let Some(shape) = peek.text.shape {
                    parts.push(lines_text(shape.lines));
                }
            }
            Body::Loading(_) | Body::Message(_) => parts.push(self.kind.label().to_string()),
        }
        if let Some(size) = self.file_size {
            parts.push(format_bytes(size));
        }
        parts.extend(self.notes.iter().cloned());
        parts.join("  ·  ")
    }

    fn render_button(
        label: &str,
        mouse_state: &MouseStateHandle,
        action: FileViewerAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let button = Container::new(
            appearance
                .ui_builder()
                .button(ButtonVariant::Text, mouse_state.clone())
                .with_text_label(label.to_string())
                .build()
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                .finish(),
        )
        .with_margin_right(4.)
        .finish();
        // Every button takes the height of a header row, so that each row the buttons wrap
        // onto is one row high.
        ConstrainedBox::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(button)
                .finish(),
        )
        .with_height(HEADER_ROW_HEIGHT)
        .finish()
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = Text::new_inline(
            file_name,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(blended_colors::text_main(theme, theme.background()))
        .finish();
        let info = Text::new_inline(
            self.info_text(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(blended_colors::text_sub(theme, theme.background()))
        .with_clip(ClipConfig::end())
        .finish();
        let title_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Container::new(name).with_margin_right(12.).finish())
            .with_child(Shrinkable::new(1., info).finish())
            .finish();

        // The buttons wrap onto further rows in a pane too narrow for one.
        let mut controls = Wrap::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max);
        let button = |label, mouse_state, action| {
            Self::render_button(label, mouse_state, action, appearance)
        };
        let zoomable = matches!(self.body, Body::Image(_) | Body::Pdf(_));
        if zoomable {
            controls.add_child(button(
                "−",
                &self.buttons.zoom_out,
                FileViewerAction::ZoomOut,
            ));
            controls.add_child(button("+", &self.buttons.zoom_in, FileViewerAction::ZoomIn));
            controls.add_child(button(
                "Fit",
                &self.buttons.fit,
                FileViewerAction::FitToWindow,
            ));
            controls.add_child(button(
                "100%",
                &self.buttons.actual_size,
                FileViewerAction::ActualSize,
            ));
        }
        if matches!(self.body, Body::Pdf(_)) {
            controls.add_child(button(
                "Previous page",
                &self.buttons.previous_page,
                FileViewerAction::PreviousPage,
            ));
            controls.add_child(button(
                "Next page",
                &self.buttons.next_page,
                FileViewerAction::NextPage,
            ));
        }

        if let Some(label) = self.open_in_editor_label() {
            controls.add_child(button(
                label,
                &self.buttons.open_in_editor,
                FileViewerAction::OpenInEditor,
            ));
        }
        controls.add_child(button(
            "Open in default app",
            &self.buttons.open,
            FileViewerAction::OpenInDefaultApp,
        ));
        controls.add_child(button(
            reveal_label(),
            &self.buttons.reveal,
            FileViewerAction::RevealInFileManager,
        ));
        if quick_look::is_available() {
            controls.add_child(button(
                "Quick Look",
                &self.buttons.quick_look,
                FileViewerAction::QuickLook,
            ));
        }
        let title_row = ConstrainedBox::new(Clipped::new(title_row).finish())
            .with_height(HEADER_ROW_HEIGHT)
            .finish();
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(title_row)
                .with_child(controls.finish())
                .finish(),
        )
        .with_horizontal_padding(12.)
        .with_vertical_padding(4.)
        .finish()
    }

    /// Shows `message` in the middle of the viewer, over a busy indicator when `busy` is set.
    fn render_message(
        &self,
        message: &str,
        busy: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let dot_color = blended_colors::text_sub(theme, theme.background());
        let active_dot_color = blended_colors::text_main(theme, theme.background());
        let since = self.loading_since;
        let text = Text::new(
            message.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(blended_colors::text_sub(theme, theme.background()))
        .finish();
        let centred = Align::new(ConstrainedBox::new(text).with_max_width(480.).finish()).finish();
        let canvas = ViewerCanvas::new(self.canvas.clone(), theme.background().into_solid())
            .with_child(centred);
        if !busy {
            return canvas.finish();
        }
        canvas
            .with_painter(move |viewport| {
                let step = since.elapsed().as_millis() / BUSY_STEP.as_millis();
                let active = (step % BUSY_DOTS as u128) as usize;
                let shapes = busy_dots(viewport.size)
                    .into_iter()
                    .enumerate()
                    .map(|(index, rect)| Shape::Fill {
                        rect: to_rect_f(rect),
                        color: if index == active {
                            active_dot_color
                        } else {
                            dot_color
                        },
                    })
                    .collect();
                Frame {
                    backdrop: None,
                    shapes,
                    repaint_after: Some(BUSY_STEP),
                }
            })
            .finish()
    }

    fn render_peek(&self, peek: &PeekBody, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let background = theme.background().into_solid();
        let text_color = blended_colors::text_main(theme, theme.background());
        let muted_color = blended_colors::text_sub(theme, theme.background());
        let font_size = appearance.monospace_font_size();
        let char_width = font_size * MONOSPACE_ADVANCE;
        let viewport = self.viewport().size;
        let number_width = row_number_width(peek.rows.len(), font_size);
        // Only what fits is laid out, so that a row costs the same however long its line is.
        let skipped_chars = (peek.scroll.x / char_width) as usize;
        let shown_chars = ((viewport.width - number_width) / char_width)
            .ceil()
            .max(0.) as usize;

        let cell = |text: String, color: ColorU| {
            Text::new_inline(text, appearance.monospace_font_family(), font_size)
                .with_color(color)
                .with_clip(ClipConfig::end())
                .finish()
        };
        let mut rows = Flex::column().with_main_axis_size(MainAxisSize::Min);
        // One more than fits, so that the space under the last whole row is not left blank.
        let shown_rows = peek_visible_rows(viewport.height) + 1;
        let first = peek.scroll.top_row;
        for index in first..(first + shown_rows).min(peek.rows.len()) {
            let is_line = index < peek.text.peek.lines.len();
            let number = if is_line {
                (index + 1).to_string()
            } else {
                String::new()
            };
            let text = peek.rows[index]
                .chars()
                .skip(skipped_chars)
                .take(shown_chars)
                .collect();
            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(
                        Container::new(cell(number, muted_color))
                            .with_horizontal_padding(TABLE_CELL_PADDING)
                            .finish(),
                    )
                    .with_width(number_width)
                    .finish(),
                )
                .with_child(cell(text, if is_line { text_color } else { muted_color }))
                .finish();
            rows.add_child(
                ConstrainedBox::new(row)
                    .with_height(PEEK_ROW_HEIGHT)
                    .finish(),
            );
        }

        let notice = Container::new(
            Text::new(
                peek.notice.clone(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(muted_color)
            .finish(),
        )
        .with_horizontal_padding(12.)
        .with_vertical_padding(8.)
        .finish();
        let lines = ViewerCanvas::new(self.canvas.clone(), background)
            .with_child(Align::new(rows.finish()).top_left().finish())
            .finish();
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(notice)
            .with_child(Expanded::new(1., lines).finish())
            .finish()
    }

    fn render_image(&self, image: &ImageBody, appearance: &Appearance) -> Box<dyn Element> {
        let background = appearance.theme().background().into_solid();
        let frames = image.frames.clone();
        let content = image.content;
        let started = image.started;
        let transform = self.transform;
        let backdrop = image.has_transparency.then(|| self.backdrop.clone());

        ViewerCanvas::new(self.canvas.clone(), background)
            .with_painter(move |viewport| {
                let placement = Placement::new(&transform, content, viewport);
                let mut frame = Frame::default();

                if let Some(backdrop) = &backdrop {
                    let area = placement.place(Rect::new(0., 0., content.width, content.height));
                    let pattern = scene::backdrop(area, viewport.size, BACKDROP_TILE);
                    frame.backdrop = Some(BackdropTiles {
                        clip: to_rect_f(pattern.clip),
                        tiles: pattern.tiles.into_iter().map(to_rect_f).collect(),
                        image: backdrop.clone(),
                    });
                }

                let (index, until_next) = if frames.len() > 1 {
                    let elapsed = started.elapsed().as_millis() as u64;
                    image_decoding::frame_at(frames.iter().map(|frame| frame.delay_ms), elapsed)
                } else {
                    (0, u64::MAX)
                };
                if frames.len() > 1 {
                    frame.repaint_after = Some(Duration::from_millis(until_next.max(1)));
                }
                if let Some(shown) = frames.get(index) {
                    let tiles = scene::image_tiles(&shown.pyramid, &placement);
                    frame
                        .shapes
                        .extend(tiles.into_iter().map(|(rect, image)| Shape::Image {
                            rect: to_rect_f(rect),
                            image: image.clone(),
                        }));
                }
                frame
            })
            .finish()
    }

    fn render_pdf(&self, pdf: &PdfBody, appearance: &Appearance) -> Box<dyn Element> {
        let background = appearance.theme().surface_2().into_solid();
        let layout = pdf.layout.clone();
        let transform = self.transform;
        // Rasters at the scale the view calls for are drawn last, over any stale ones.
        let viewport = self.viewport();
        let mut strips: Vec<(bool, Rect, Arc<StaticImage>)> = pdf
            .strips
            .iter()
            .map(|(key, (strip, image))| {
                let current = scene::is_at_wanted_scale(key, &layout, &transform, viewport);
                (current, strip.rect, image.clone())
            })
            .collect();
        strips.sort_by_key(|(current, _, _)| *current);

        ViewerCanvas::new(self.canvas.clone(), background)
            .with_painter(move |viewport| {
                let placement = Placement::new(&transform, layout.content_size(), viewport);
                let mut frame = Frame::default();
                let paper = scene::page_rects(&layout, &placement);
                frame
                    .shapes
                    .extend(paper.into_iter().map(|rect| Shape::Fill {
                        rect: to_rect_f(rect),
                        color: ColorU::white(),
                    }));
                let visible = placement.visible();
                frame.shapes.extend(
                    strips
                        .iter()
                        .filter(|(_, rect, _)| rect.intersects(visible))
                        .map(|(_, rect, image)| Shape::Image {
                            rect: to_rect_f(placement.place(*rect)),
                            image: image.clone(),
                        }),
                );
                frame
            })
            .finish()
    }

    fn render_table(&self, body: &TableBody, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let background = theme.background().into_solid();
        let header_background = theme.surface_2().into_solid();
        let text_color = blended_colors::text_main(theme, theme.background());
        let muted_color = blended_colors::text_sub(theme, theme.background());
        let viewport = self.viewport().size;
        let table = &body.table;

        let number_width = row_number_width(table.row_count(), appearance.monospace_font_size());
        let (columns, _) = csv::visible_columns(
            &body.widths,
            body.scroll.x,
            (viewport.width - number_width).max(0.),
        );
        let cell = |text: String, width: f32, color: ColorU| -> Box<dyn Element> {
            ConstrainedBox::new(
                Container::new(
                    Text::new_inline(
                        text,
                        appearance.monospace_font_family(),
                        appearance.monospace_font_size(),
                    )
                    .with_color(color)
                    .with_clip(ClipConfig::end())
                    .finish(),
                )
                .with_horizontal_padding(TABLE_CELL_PADDING)
                .with_vertical_padding(4.)
                .finish(),
            )
            .with_width(width)
            .with_height(TABLE_ROW_HEIGHT)
            .finish()
        };
        let row = |label: String, fields: Vec<std::borrow::Cow<'_, str>>, color: ColorU| {
            let mut row = Flex::row().with_main_axis_size(MainAxisSize::Min);
            row.add_child(cell(label, number_width, muted_color));
            for column in columns.clone() {
                let text = fields.get(column).map(|field| cell_text(field));
                let width = body.widths.get(column).copied().unwrap_or_default();
                row.add_child(cell(text.unwrap_or_default(), width, color));
            }
            row.finish()
        };

        let mut rows = Flex::column().with_main_axis_size(MainAxisSize::Min);
        rows.add_child(
            Container::new(row(String::new(), table.header(), text_color))
                .with_background_color(header_background)
                .finish(),
        );
        // One more than fits, so that the space under the last whole row is not left blank.
        let shown = csv::visible_rows(viewport.height, TABLE_ROW_HEIGHT) + 1;
        let first = body.scroll.top_row;
        for index in first..(first + shown).min(table.row_count()) {
            rows.add_child(row((index + 1).to_string(), table.row(index), text_color));
        }

        ViewerCanvas::new(self.canvas.clone(), background)
            .with_child(Align::new(rows.finish()).top_left().finish())
            .finish()
    }
}

/// Where the dots of the busy indicator go in a viewer of size `viewport`: in a row under the
/// message in its middle.
fn busy_dots(viewport: Size) -> Vec<Rect> {
    let pitch = BUSY_DOT_SIZE + BUSY_DOT_GAP;
    let width = BUSY_DOTS as f32 * pitch - BUSY_DOT_GAP;
    let left = (viewport.width - width) / 2.;
    let top = viewport.height / 2. + BUSY_DOTS_BELOW_CENTRE;
    (0..BUSY_DOTS)
        .map(|dot| Rect::new(left + dot as f32 * pitch, top, BUSY_DOT_SIZE, BUSY_DOT_SIZE))
        .collect()
}

fn default_zoom_mode(kind: ViewerKind) -> ZoomMode {
    match kind {
        ViewerKind::Pdf | ViewerKind::Office => ZoomMode::FitWidth,
        ViewerKind::Image
        | ViewerKind::Svg
        | ViewerKind::Table
        | ViewerKind::Binary
        | ViewerKind::LargeText
        | ViewerKind::NotLoaded => ZoomMode::Fit,
    }
}

/// How many peeked lines are fully visible in `height` pixels.
fn peek_visible_rows(height: f32) -> usize {
    (height / PEEK_ROW_HEIGHT).floor().max(0.) as usize
}

/// The rows a peek is drawn as: its lines, on one line each, and a last row saying that the
/// file goes on when it does.
fn peek_rows(text: &LargeText) -> Vec<String> {
    let mut rows: Vec<String> = text
        .peek
        .lines
        .iter()
        .map(|line| {
            let mut row: String = line
                .text
                .chars()
                .map(|character| {
                    if character.is_control() {
                        ' '
                    } else {
                        character
                    }
                })
                .collect();
            if line.truncated {
                row.push_str(LINE_TRUNCATED_MARKER);
            }
            row
        })
        .collect();
    if text.peek.has_more {
        rows.push("… the rest of the file is not shown".to_string());
    }
    rows
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Reveal in Explorer"
    } else {
        "Reveal in file manager"
    }
}

fn to_texture(image: RgbaImage) -> Arc<StaticImage> {
    Arc::new(StaticImage::from_rgba(image))
}

fn to_rect_f(rect: Rect) -> RectF {
    RectF::new(vec2f(rect.x, rect.y), vec2f(rect.width, rect.height))
}

/// Width of the column of row numbers for a table of `row_count` rows set in a monospace font
/// of `font_size`.
fn row_number_width(row_count: usize, font_size: f32) -> f32 {
    let digits = row_count.max(1).to_string().len().max(2);
    digits as f32 * font_size * MONOSPACE_ADVANCE + 2. * TABLE_CELL_PADDING
}

/// A cell's text as it is laid out: on one line and of bounded length.
fn cell_text(field: &str) -> String {
    field
        .chars()
        .take(TABLE_MAX_CELL_CHARS)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn render_svg(
    path: &Path,
    text: Option<Arc<str>>,
    pixels_per_unit: f32,
) -> Result<(Arc<str>, SvgRaster), String> {
    let text = match text {
        Some(text) => text,
        None => fs::read_to_string(path)
            .map(Arc::from)
            .map_err(|err| format!("The file could not be read: {err}"))?,
    };
    let raster = svg::render(&text, pixels_per_unit).map_err(|err| err.to_string())?;
    Ok((text, raster))
}

fn read_table(path: &Path, text: Option<Arc<str>>) -> Result<Arc<Table>, String> {
    let text: Arc<str> = match text {
        Some(text) => text,
        None => fs::read_to_string(path)
            .map(Arc::from)
            .map_err(|err| format!("The file could not be read as text: {err}"))?,
    };
    Ok(Arc::new(Table::parse(text, csv::delimiter_for_path(path))))
}

impl Entity for FileViewerView {
    type Event = FileViewerEvent;
}

impl View for FileViewerView {
    fn ui_name() -> &'static str {
        "FileViewerView"
    }

    fn keymap_context(&self, _: &AppContext) -> keymap::Context {
        let mut context = Self::default_keymap_context();
        match self.body {
            Body::Image(_) | Body::Pdf(_) => {
                context.set.insert(ZOOMABLE_CONTEXT);
            }
            Body::Loading(_) | Body::Message(_) | Body::Peek(_) | Body::Table(_) => {}
        }
        context
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let body = match &self.body {
            Body::Loading(message) => self.render_message(message, true, appearance),
            Body::Message(message) => self.render_message(message, false, appearance),
            Body::Peek(peek) => self.render_peek(peek, appearance),
            Body::Image(image) => self.render_image(image, appearance),
            Body::Pdf(pdf) => self.render_pdf(pdf, appearance),
            Body::Table(table) => self.render_table(table, appearance),
        };
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_header(appearance))
            .with_child(Expanded::new(1., body).finish())
            .finish()
    }
}

impl TypedActionView for FileViewerView {
    type Action = FileViewerAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FileViewerAction::OpenInEditor => ctx.emit(FileViewerEvent::OpenInEditorRequested),
            FileViewerAction::OpenInDefaultApp => ctx.open_file_path(&self.path),
            FileViewerAction::RevealInFileManager => ctx.open_file_path_in_explorer(&self.path),
            FileViewerAction::QuickLook => {
                if let Err(err) = quick_look::show(&self.path) {
                    log::warn!("Could not open Quick Look for {:?}: {err}", self.path);
                }
            }
            FileViewerAction::ZoomIn => self.step_zoom(true, ctx),
            FileViewerAction::ZoomOut => self.step_zoom(false, ctx),
            FileViewerAction::ActualSize => self.set_zoom_mode(ZoomMode::Manual(1.), ctx),
            FileViewerAction::FitToWindow => {
                self.set_zoom_mode(default_zoom_mode(self.kind), ctx);
            }
            FileViewerAction::PreviousPage => self.step_page(false, ctx),
            FileViewerAction::NextPage => self.step_page(true, ctx),
            FileViewerAction::PageDigit(digit) => {
                if let Body::Pdf(pdf) = &mut self.body
                    && pdf.page_entry.len() < 6
                {
                    pdf.page_entry.push(char::from(b'0' + digit));
                    ctx.notify();
                }
            }
            FileViewerAction::GoToTypedPage => {
                let Body::Pdf(pdf) = &mut self.body else {
                    return;
                };
                let typed = std::mem::take(&mut pdf.page_entry);
                match typed.parse::<usize>() {
                    Ok(page) if page >= 1 => self.go_to_page(page - 1, ctx),
                    Ok(_) | Err(_) => ctx.notify(),
                }
            }
            FileViewerAction::CancelPageEntry => {
                if let Body::Pdf(pdf) = &mut self.body {
                    pdf.page_entry.clear();
                    ctx.notify();
                }
            }
            FileViewerAction::Wheel {
                position,
                delta,
                precise,
                zoom,
            } => {
                if *zoom {
                    let Some(content) = self.content_size() else {
                        return;
                    };
                    let viewport = self.viewport().size;
                    let pixels = geometry::wheel_scroll(Offset::new(0., delta.y()), *precise);
                    let scale = self.transform.scale(content, viewport)
                        * geometry::wheel_zoom_factor(-pixels.y);
                    self.zoom_to(scale, Offset::new(position.x(), position.y()), ctx);
                } else {
                    let delta = geometry::wheel_scroll(Offset::new(delta.x(), delta.y()), *precise);
                    self.scroll_by(delta, ctx);
                }
            }
            FileViewerAction::Pressed {
                position,
                click_count,
            } => {
                ctx.focus_self();
                ctx.emit(FileViewerEvent::Focused);
                self.reload_if_changed(ctx);
                // A double click switches between the fitted view and the natural size.
                if *click_count == 2
                    && let Some(content) = self.content_size()
                {
                    let viewport = self.viewport().size;
                    let fitted = ViewTransform::new(default_zoom_mode(self.kind));
                    let is_fitted = (self.transform.scale(content, viewport)
                        - fitted.scale(content, viewport))
                    .abs()
                        < 1e-3;
                    if is_fitted {
                        self.zoom_to(1., Offset::new(position.x(), position.y()), ctx);
                    } else {
                        self.set_zoom_mode(default_zoom_mode(self.kind), ctx);
                    }
                }
            }
            FileViewerAction::Dragged { delta } => {
                self.scroll_by(Offset::new(delta.x(), delta.y()), ctx);
            }
            FileViewerAction::ViewportChanged => self.view_changed(true, ctx),
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "scene_tests.rs"]
mod scene_tests;
