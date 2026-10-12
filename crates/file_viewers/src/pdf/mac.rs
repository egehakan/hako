//! PDF rasterising with CoreGraphics.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use futures::channel::oneshot;
use image::RgbaImage;
use objc2_core_foundation::{CFRetained, CFURL, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGContext, CGPDFBox, CGPDFDocument, CGPDFPage};

use super::{OpenedPdf, PageGeometry, PdfError, Strip};
use crate::bitmap::Bitmap;

struct RenderJob {
    strip: Strip,
    cancelled: Arc<AtomicBool>,
    reply: oneshot::Sender<Result<RgbaImage, PdfError>>,
}

/// Rasterises strips of one document on a thread of its own.
///
/// The document is opened, used and released on that thread only, so nothing depends on
/// CoreGraphics documents being safe to share between threads.
#[derive(Clone)]
pub struct PdfWorker {
    jobs: mpsc::Sender<RenderJob>,
}

impl PdfWorker {
    /// Opens the document at `path`, unlocking it with `password` if it is protected.
    pub fn open(
        path: PathBuf,
        password: Option<String>,
    ) -> impl Future<Output = Result<OpenedPdf, PdfError>> + Send + 'static {
        let (opened_sender, opened) = oneshot::channel();
        let (jobs, job_receiver) = mpsc::channel::<RenderJob>();
        let spawned = thread::Builder::new()
            .name("pdf-rasteriser".to_string())
            .spawn(move || {
                let document = match Document::open(&path, password.as_deref()) {
                    Ok(document) => document,
                    Err(err) => {
                        let _ = opened_sender.send(Err(err));
                        return;
                    }
                };
                if opened_sender.send(Ok(document.pages())).is_err() {
                    return;
                }
                // Ends when every handle to the worker is gone.
                for job in job_receiver {
                    if job.cancelled.load(Ordering::Relaxed) {
                        let _ = job.reply.send(Err(PdfError::Cancelled));
                        continue;
                    }
                    let _ = job.reply.send(document.render(&job.strip));
                }
            });

        async move {
            if spawned.is_err() {
                return Err(PdfError::Unreadable);
            }
            let pages = opened.await.map_err(|_| PdfError::Unreadable)??;
            Ok(OpenedPdf {
                worker: PdfWorker { jobs },
                pages,
            })
        }
    }

    /// Rasterises `strip`. Setting `cancelled` before the worker gets to it skips the work.
    pub fn render(
        &self,
        strip: Strip,
        cancelled: Arc<AtomicBool>,
    ) -> impl Future<Output = Result<RgbaImage, PdfError>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let sent = self.jobs.send(RenderJob {
            strip,
            cancelled,
            reply,
        });
        async move {
            sent.map_err(|_| PdfError::RenderFailed)?;
            result.await.map_err(|_| PdfError::RenderFailed)?
        }
    }
}

struct Document {
    document: CFRetained<CGPDFDocument>,
}

impl Document {
    fn open(path: &Path, password: Option<&str>) -> Result<Self, PdfError> {
        let url = CFURL::from_file_path(path).ok_or(PdfError::Unreadable)?;
        let document = CGPDFDocument::with_url(Some(&url)).ok_or(PdfError::Unreadable)?;

        if !CGPDFDocument::is_unlocked(Some(&document)) {
            let Some(password) = password else {
                return Err(PdfError::PasswordRequired);
            };
            let password = CString::new(password).map_err(|_| PdfError::WrongPassword)?;
            let password_ptr =
                NonNull::new(password.as_ptr().cast_mut()).ok_or(PdfError::WrongPassword)?;
            // SAFETY: `password_ptr` points to a NUL-terminated string that outlives the call
            // and is only read.
            let unlocked =
                unsafe { CGPDFDocument::unlock_with_password(Some(&document), password_ptr) };
            if !unlocked {
                return Err(PdfError::WrongPassword);
            }
        }
        if CGPDFDocument::number_of_pages(Some(&document)) == 0 {
            return Err(PdfError::Empty);
        }
        Ok(Self { document })
    }

    /// The page at `index`, counted from zero.
    fn page(&self, index: usize) -> Option<CFRetained<CGPDFPage>> {
        CGPDFDocument::page(Some(&self.document), index + 1)
    }

    fn pages(&self) -> Vec<PageGeometry> {
        let count = CGPDFDocument::number_of_pages(Some(&self.document));
        (0..count)
            .map(|index| {
                self.page(index)
                    .map(|page| page_geometry(&page))
                    // A page that cannot be read still takes its place in the column.
                    .unwrap_or(PageGeometry {
                        width: 612.,
                        height: 792.,
                    })
            })
            .collect()
    }

    fn render(&self, strip: &Strip) -> Result<RgbaImage, PdfError> {
        let page = self
            .page(strip.key.page as usize)
            .ok_or(PdfError::RenderFailed)?;
        let geometry = page_geometry(&page);
        let bitmap =
            Bitmap::new(strip.pixel_width, strip.pixel_height).ok_or(PdfError::RenderFailed)?;
        let context = bitmap.context();

        let strip_bounds = CGRect::new(
            CGPoint::new(0., 0.),
            CGSize::new(strip.pixel_width.into(), strip.pixel_height.into()),
        );
        CGContext::set_rgb_fill_color(Some(context), 1., 1., 1., 1.);
        CGContext::fill_rect(Some(context), strip_bounds);

        // CoreGraphics measures from the bottom of the page, strips from its top.
        let scale = f64::from(strip.scale);
        let page_pixel_height = f64::from(geometry.height) * scale;
        let strip_bottom = page_pixel_height - f64::from(strip.pixel_top + strip.pixel_height);
        CGContext::translate_ctm(Some(context), 0., -strip_bottom);
        CGContext::scale_ctm(Some(context), scale, scale);

        // The drawing transform never enlarges, so it is asked for the page at its natural
        // size, which leaves it to apply only the rotation and the origin of the crop box.
        let natural = CGRect::new(
            CGPoint::new(0., 0.),
            CGSize::new(geometry.width.into(), geometry.height.into()),
        );
        let transform =
            CGPDFPage::drawing_transform(Some(&page), CGPDFBox::CropBox, natural, 0, true);
        CGContext::concat_ctm(Some(context), transform);
        CGContext::draw_pdf_page(Some(context), Some(&page));

        bitmap.into_image().ok_or(PdfError::RenderFailed)
    }
}

fn page_geometry(page: &CGPDFPage) -> PageGeometry {
    let crop_box = CGPDFPage::box_rect(Some(page), CGPDFBox::CropBox);
    let quarter_turns = CGPDFPage::rotation_angle(Some(page)).rem_euclid(360) / 90;
    let (width, height) = if quarter_turns % 2 == 1 {
        (crop_box.size.height, crop_box.size.width)
    } else {
        (crop_box.size.width, crop_box.size.height)
    };
    PageGeometry {
        width: (width as f32).max(1.),
        height: (height as f32).max(1.),
    }
}

#[cfg(test)]
#[path = "mac_tests.rs"]
mod tests;
