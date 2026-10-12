use std::fs;

use futures::executor::block_on;
use objc2_core_foundation::{CFDictionary, CFString, CFType};
use objc2_core_graphics::{
    CGPDFContextClose, CGPDFContextCreateWithURL, kCGPDFContextOwnerPassword,
    kCGPDFContextUserPassword,
};

use super::super::{PageLayout, STRIP_HEIGHT, strips_for_page};
use super::*;
use crate::geometry::Rect;

const EVERYTHING: Rect = Rect::new(0., 0., 1e6, 1e6);

/// Writes a PDF with one page per entry of `pages`, each `(width, height)` in points and
/// white with a red rectangle over its top-left quarter.
fn write_pdf(path: &Path, pages: &[(f64, f64)], password: Option<&str>) {
    let url = CFURL::from_file_path(path).unwrap();
    let password = password.map(CFString::from_str);
    let owner_password = CFString::from_str("owner");
    let options = password.as_ref().map(|password| {
        // SAFETY: the keys are constants of CoreGraphics, valid for the lifetime of the process.
        let keys = unsafe { [kCGPDFContextUserPassword, kCGPDFContextOwnerPassword] };
        let values: [&CFType; 2] = [password.as_ref(), owner_password.as_ref()];
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    });
    // SAFETY: a null media box is allowed, and the options are a dictionary of documented keys.
    let context = unsafe {
        CGPDFContextCreateWithURL(
            Some(&url),
            std::ptr::null(),
            options.as_ref().map(|options| options.as_opaque()),
        )
    }
    .unwrap();
    for (width, height) in pages {
        let media_box = CGRect::new(CGPoint::new(0., 0.), CGSize::new(*width, *height));
        // SAFETY: `media_box` is a valid rectangle that outlives the call.
        unsafe { CGContext::begin_page(Some(&context), &media_box) };
        CGContext::set_rgb_fill_color(Some(&context), 1., 0., 0., 1.);
        // The origin of a page is its bottom-left corner.
        CGContext::fill_rect(
            Some(&context),
            CGRect::new(
                CGPoint::new(0., height / 2.),
                CGSize::new(width / 2., height / 2.),
            ),
        );
        CGContext::end_page(Some(&context));
    }
    CGPDFContextClose(Some(&context));
}

fn open(path: &Path, password: Option<&str>) -> Result<OpenedPdf, PdfError> {
    block_on(PdfWorker::open(
        path.to_path_buf(),
        password.map(str::to_string),
    ))
}

fn render(worker: &PdfWorker, strip: Strip) -> Result<RgbaImage, PdfError> {
    block_on(worker.render(strip, Arc::new(AtomicBool::new(false))))
}

fn is_red(pixel: &image::Rgba<u8>) -> bool {
    pixel.0[0] > 200 && pixel.0[1] < 60 && pixel.0[2] < 60 && pixel.0[3] == 255
}

fn is_white(pixel: &image::Rgba<u8>) -> bool {
    pixel.0 == [255, 255, 255, 255]
}

#[test]
fn opens_a_document_and_measures_its_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-pages.pdf");
    write_pdf(&path, &[(200., 100.), (120., 300.)], None);

    let opened = open(&path, None).unwrap();
    assert_eq!(
        opened.pages,
        vec![
            PageGeometry {
                width: 200.,
                height: 100.
            },
            PageGeometry {
                width: 120.,
                height: 300.
            },
        ]
    );
}

#[test]
fn rasterises_a_page_upright_at_the_requested_scale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-pages.pdf");
    write_pdf(&path, &[(200., 100.), (120., 300.)], None);
    let opened = open(&path, None).unwrap();
    let layout = PageLayout::new(&opened.pages);

    let strips = strips_for_page(&layout, 0, 2., EVERYTHING);
    assert_eq!(strips.len(), 1);
    let image = render(&opened.worker, strips[0]).unwrap();

    assert_eq!(image.dimensions(), (400, 200));
    assert!(is_red(image.get_pixel(50, 25)));
    assert!(is_red(image.get_pixel(190, 90)));
    assert!(is_white(image.get_pixel(210, 25)));
    assert!(is_white(image.get_pixel(50, 110)));
    assert!(is_white(image.get_pixel(390, 190)));
}

#[test]
fn a_tall_page_comes_out_in_strips_that_line_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tall.pdf");
    write_pdf(&path, &[(100., 1200.)], None);
    let opened = open(&path, None).unwrap();
    let layout = PageLayout::new(&opened.pages);

    // 1200 points at 4 pixels per point is 4800 pixels, red down to pixel 2400.
    let strips = strips_for_page(&layout, 0, 4., EVERYTHING);
    assert_eq!(strips.len(), 3);
    let images: Vec<RgbaImage> = strips
        .iter()
        .map(|strip| render(&opened.worker, *strip).unwrap())
        .collect();

    assert_eq!(images[0].dimensions(), (400, STRIP_HEIGHT));
    assert_eq!(images[2].dimensions(), (400, 4800 - 2 * STRIP_HEIGHT));
    assert!(is_red(images[0].get_pixel(10, 10)));
    assert!(is_red(images[0].get_pixel(10, STRIP_HEIGHT - 1)));
    // The red area ends 352 pixels into the second strip.
    assert!(is_red(images[1].get_pixel(10, 340)));
    assert!(is_white(images[1].get_pixel(10, 360)));
    assert!(is_white(images[2].get_pixel(10, 10)));
    assert!(is_white(images[0].get_pixel(300, 10)));
}

#[test]
fn a_cancelled_request_is_not_rasterised() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.pdf");
    write_pdf(&path, &[(200., 100.)], None);
    let opened = open(&path, None).unwrap();
    let layout = PageLayout::new(&opened.pages);
    let strip = strips_for_page(&layout, 0, 1., EVERYTHING)[0];

    let cancelled = Arc::new(AtomicBool::new(true));
    let result = block_on(opened.worker.render(strip, cancelled));
    assert_eq!(result.unwrap_err(), PdfError::Cancelled);
}

#[test]
fn a_page_that_does_not_exist_fails_without_ending_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.pdf");
    write_pdf(&path, &[(200., 100.)], None);
    let opened = open(&path, None).unwrap();
    let layout = PageLayout::new(&[opened.pages[0], opened.pages[0]]);

    let missing = strips_for_page(&layout, 1, 1., EVERYTHING)[0];
    assert_eq!(
        render(&opened.worker, missing).unwrap_err(),
        PdfError::RenderFailed
    );
    let present = strips_for_page(&layout, 0, 1., EVERYTHING)[0];
    assert!(render(&opened.worker, present).is_ok());
}

#[test]
fn a_protected_document_needs_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.pdf");
    write_pdf(&path, &[(200., 100.)], Some("sesame"));

    assert_eq!(open(&path, None).err(), Some(PdfError::PasswordRequired));
    assert_eq!(
        open(&path, Some("guess")).err(),
        Some(PdfError::WrongPassword)
    );
    let opened = open(&path, Some("sesame")).unwrap();
    assert_eq!(opened.pages.len(), 1);
    let layout = PageLayout::new(&opened.pages);
    let image = render(
        &opened.worker,
        strips_for_page(&layout, 0, 1., EVERYTHING)[0],
    )
    .unwrap();
    assert!(is_red(image.get_pixel(10, 10)));
}

#[test]
fn a_damaged_or_missing_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let garbage = dir.path().join("garbage.pdf");
    fs::write(&garbage, b"%PDF-1.7\nthis is not really a document").unwrap();

    assert_eq!(open(&garbage, None).err(), Some(PdfError::Unreadable));
    assert_eq!(
        open(&dir.path().join("missing.pdf"), None).err(),
        Some(PdfError::Unreadable)
    );
}
