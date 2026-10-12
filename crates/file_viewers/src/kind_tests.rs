use std::fs;
use std::path::Path;

use super::*;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

#[test]
fn extension_picks_the_viewer_case_insensitively() {
    assert_eq!(
        ViewerKind::for_extension(Path::new("a/photo.JPG")),
        Some(ViewerKind::Image)
    );
    assert_eq!(
        ViewerKind::for_extension(Path::new("logo.svg")),
        Some(ViewerKind::Svg)
    );
    assert_eq!(
        ViewerKind::for_extension(Path::new("data.Tsv")),
        Some(ViewerKind::Table)
    );
    assert_eq!(ViewerKind::for_extension(Path::new("main.rs")), None);
    assert_eq!(ViewerKind::for_extension(Path::new("Makefile")), None);
    assert_eq!(ViewerKind::for_extension(Path::new(".png")), None);
}

#[test]
fn system_formats_have_a_viewer_only_on_macos() {
    let expected = |kind| cfg!(target_os = "macos").then_some(kind);
    assert_eq!(
        ViewerKind::for_extension(Path::new("paper.pdf")),
        expected(ViewerKind::Pdf)
    );
    assert_eq!(
        ViewerKind::for_extension(Path::new("report.docx")),
        expected(ViewerKind::Office)
    );
    assert_eq!(
        ViewerKind::for_extension(Path::new("IMG_0001.HEIC")),
        expected(ViewerKind::Image)
    );
}

#[test]
fn only_viewers_without_a_text_form_take_the_place_of_an_editor() {
    assert!(opens_in_viewer(Path::new("photo.png")));
    assert!(!opens_in_viewer(Path::new("logo.svg")));
    assert!(!opens_in_viewer(Path::new("data.csv")));
    assert!(!opens_in_viewer(Path::new("archive.zip")));
    assert!(!opens_in_viewer(Path::new("notes.txt")));
}

#[test]
fn content_overrides_an_unlisted_extension() {
    assert_eq!(
        classify_content(Path::new("download.dat"), PNG_MAGIC),
        Some(ViewerKind::Image)
    );
    assert_eq!(
        classify_content(Path::new("anim"), b"GIF89a\x01\0\x01\0"),
        Some(ViewerKind::Image)
    );
    assert_eq!(
        classify_content(Path::new("x.bin"), b"RIFF\x10\0\0\0WEBPVP8 "),
        Some(ViewerKind::Image)
    );
}

#[test]
fn unlisted_binary_content_gets_the_binary_viewer() {
    assert_eq!(
        classify_content(Path::new("model.parquet"), b"PAR1\0\x15\x04\x15"),
        Some(ViewerKind::Binary)
    );
    assert_eq!(
        classify_content(Path::new("blob"), &[0xff, 0xfe, 0x41, 0x00]),
        Some(ViewerKind::Binary)
    );
    assert_eq!(
        classify_content(Path::new("notes.txt"), b"plain text"),
        None
    );
    assert_eq!(classify_content(Path::new("empty"), b""), None);
}

#[test]
fn text_under_an_image_extension_opens_as_text() {
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 12\n";
    assert_eq!(classify_content(Path::new("photo.png"), pointer), None);
    assert_eq!(
        classify_content(Path::new("photo.bmp"), b"BM\x46\0\0\0\0\0\0\0"),
        Some(ViewerKind::Image)
    );
}

#[test]
fn text_backed_kinds_need_text_content() {
    assert_eq!(
        classify_content(
            Path::new("logo.svg"),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>"
        ),
        Some(ViewerKind::Svg)
    );
    assert_eq!(
        classify_content(Path::new("data.csv"), b"a,b\n1,2\n"),
        Some(ViewerKind::Table)
    );
    assert_eq!(
        classify_content(Path::new("data.csv"), b"a,b\0\x01\x02"),
        Some(ViewerKind::Binary)
    );
}

#[test]
fn a_multibyte_character_cut_by_the_window_is_still_text() {
    let text = "héllo wörld €".as_bytes();
    assert!(!is_binary(&text[..text.len() - 1]));
    assert!(is_binary(b"caf\xe9 au lait"));
}

#[cfg(target_os = "macos")]
#[test]
fn pdf_and_office_files_are_recognised() {
    assert_eq!(
        classify_content(Path::new("scan"), b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"),
        Some(ViewerKind::Pdf)
    );
    assert_eq!(
        classify_content(Path::new("memo.rtf"), b"{\\rtf1\\ansi hello}"),
        Some(ViewerKind::Office)
    );
    assert_eq!(
        classify_content(Path::new("book.xlsx"), b"PK\x03\x04\x14\0\x06\0"),
        Some(ViewerKind::Office)
    );
}

#[test]
fn classify_reads_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("picture.unknown");
    fs::write(&image, PNG_MAGIC).unwrap();
    let text = dir.path().join("readme");
    fs::write(&text, "hello\n").unwrap();

    assert_eq!(classify(&image), Some(ViewerKind::Image));
    assert_eq!(classify(&text), None);
    assert_eq!(classify(&dir.path().join("not-created-yet.png")), None);
}
