//! Which viewer a file gets.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// How many leading bytes of a file are read to tell what it holds.
const SNIFF_LEN: usize = 8192;

/// Raster formats decoded on every platform.
const PORTABLE_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
];

/// Raster formats that only the system decoder of macOS reads.
const SYSTEM_IMAGE_EXTENSIONS: &[&str] = &["heic", "heif", "avif", "icns"];

const TABLE_EXTENSIONS: &[&str] = &["csv", "tsv"];

const OFFICE_EXTENSIONS: &[&str] = &[
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "rtf", "odt", "pages", "numbers", "key",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewerKind {
    /// A raster image.
    Image,
    Svg,
    Pdf,
    /// Delimited text shown as a table.
    Table,
    /// A document shown through a conversion to PDF.
    Office,
    /// A file with no text form and no dedicated viewer.
    Binary,
}

impl ViewerKind {
    /// The viewer for files with the extension of `path`, if this platform has one. Reads
    /// nothing from disk.
    pub fn for_extension(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let extension = extension.as_str();
        let kind = if PORTABLE_IMAGE_EXTENSIONS.contains(&extension) {
            Self::Image
        } else if SYSTEM_IMAGE_EXTENSIONS.contains(&extension) {
            Self::Image.only_on_macos()?
        } else if extension == "svg" {
            Self::Svg
        } else if extension == "pdf" {
            Self::Pdf.only_on_macos()?
        } else if TABLE_EXTENSIONS.contains(&extension) {
            Self::Table
        } else if OFFICE_EXTENSIONS.contains(&extension) {
            Self::Office.only_on_macos()?
        } else {
            return None;
        };
        Some(kind)
    }

    fn only_on_macos(self) -> Option<Self> {
        cfg!(target_os = "macos").then_some(self)
    }

    /// Whether the file is text that an editor can also show, with the viewer as its rendered
    /// form.
    pub fn is_text_backed(self) -> bool {
        match self {
            Self::Svg | Self::Table => true,
            Self::Image | Self::Pdf | Self::Office | Self::Binary => false,
        }
    }

    /// Name of the kind as shown next to the file name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Svg => "SVG image",
            Self::Pdf => "PDF document",
            Self::Table => "Table",
            Self::Office => "Document",
            Self::Binary => "Binary file",
        }
    }
}

/// Whether a file named like `path` is shown by a viewer that takes the place of an editor, so
/// that opening it should never hand it to an external text editor. Reads nothing from disk.
pub fn opens_in_viewer(path: &Path) -> bool {
    ViewerKind::for_extension(path).is_some_and(|kind| !kind.is_text_backed())
}

/// Decides which viewer the file at `path` gets from its extension and its first bytes, or
/// `None` when it should open as text. A file that cannot be read opens as text, which is where
/// a file that does not exist yet gets created.
pub fn classify(path: &Path) -> Option<ViewerKind> {
    let head = read_head(path)?;
    classify_content(path, &head)
}

/// [`classify`] for a file whose first bytes are already in hand.
pub fn classify_content(path: &Path, head: &[u8]) -> Option<ViewerKind> {
    if let Some(kind) = kind_from_magic(head) {
        return Some(kind);
    }
    let binary = is_binary(head);
    match ViewerKind::for_extension(path) {
        Some(ViewerKind::Office) => Some(ViewerKind::Office),
        // An image or PDF extension on a text file is a placeholder (a Git LFS pointer, an empty
        // file), which is only readable as text.
        Some(ViewerKind::Image | ViewerKind::Pdf) if binary => ViewerKind::for_extension(path),
        Some(kind @ (ViewerKind::Svg | ViewerKind::Table)) if !binary => Some(kind),
        Some(
            ViewerKind::Image
            | ViewerKind::Pdf
            | ViewerKind::Svg
            | ViewerKind::Table
            | ViewerKind::Binary,
        )
        | None => binary.then_some(ViewerKind::Binary),
    }
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut head = Vec::with_capacity(SNIFF_LEN);
    File::open(path)
        .ok()?
        .take(SNIFF_LEN as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// Recognises the formats whose viewer does not depend on the file's name.
fn kind_from_magic(head: &[u8]) -> Option<ViewerKind> {
    let image = head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(b"\xff\xd8\xff")
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"));
    if image {
        return Some(ViewerKind::Image);
    }
    if head.starts_with(b"%PDF-") {
        return ViewerKind::Pdf.only_on_macos();
    }
    None
}

/// Whether `head` is the start of something other than UTF-8 text. This is the condition under
/// which loading the file into a text buffer fails.
pub fn is_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    match std::str::from_utf8(head) {
        Ok(_) => false,
        // The window may end in the middle of a multi-byte character.
        Err(err) => err.error_len().is_some(),
    }
}

#[cfg(test)]
#[path = "kind_tests.rs"]
mod tests;
