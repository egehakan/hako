//! Stand-in for the PDF rasteriser on platforms without one.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use image::RgbaImage;

use super::{OpenedPdf, PdfError, Strip};

#[derive(Clone)]
pub struct PdfWorker;

impl PdfWorker {
    pub fn open(
        _path: PathBuf,
        _password: Option<String>,
    ) -> impl Future<Output = Result<OpenedPdf, PdfError>> + Send + 'static {
        async { Err(PdfError::Unsupported) }
    }

    pub fn render(
        &self,
        _strip: Strip,
        _cancelled: Arc<AtomicBool>,
    ) -> impl Future<Output = Result<RgbaImage, PdfError>> + Send + 'static {
        async { Err(PdfError::Unsupported) }
    }
}
