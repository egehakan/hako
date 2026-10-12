use futures::executor::block_on;

use super::*;
use crate::office::{DEFAULT_TIMEOUT, convert_cached};
use crate::pdf::PdfWorker;

#[test]
fn finds_an_html_export() {
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("report.docx.qlpreview");
    fs::create_dir(&export).unwrap();
    fs::write(export.join("Preview.html"), "<html></html>").unwrap();
    fs::write(export.join("PreviewProperties.plist"), "").unwrap();

    assert_eq!(locate_preview(dir.path()), Some(Preview::Html(export)));
}

#[test]
fn prefers_a_pdf_export() {
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("deck.key.qlpreview");
    fs::create_dir(&export).unwrap();
    fs::write(export.join("Preview.html"), "<html></html>").unwrap();
    fs::write(export.join("Preview.pdf"), "%PDF").unwrap();

    assert_eq!(
        locate_preview(dir.path()),
        Some(Preview::Pdf(export.join("Preview.pdf")))
    );
}

#[test]
fn an_export_without_a_document_is_no_preview() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(locate_preview(dir.path()), None);

    // Formats Quick Look shows with a native view export a link, not a document.
    let export = dir.path().join("memo.rtf.qlpreview");
    fs::create_dir(&export).unwrap();
    fs::write(export.join("Preview.url"), "").unwrap();
    assert_eq!(locate_preview(dir.path()), None);

    fs::write(dir.path().join("stray.qlpreview"), "a file, not a folder").unwrap();
    assert_eq!(locate_preview(dir.path()), None);
}

#[test]
fn recognises_extensions_whatever_their_case() {
    assert!(has_extension(
        Path::new("Memo.RTF"),
        TEXTUTIL_FIRST_EXTENSIONS
    ));
    assert!(!has_extension(
        Path::new("memo.docx"),
        TEXTUTIL_FIRST_EXTENSIONS
    ));
    assert!(!has_extension(Path::new("rtf"), TEXTUTIL_FIRST_EXTENSIONS));
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{:?} failed: {}",
        command.get_program(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Writes a word-processing document in the format of `extension` saying `text`.
fn write_document(dir: &Path, extension: &str, text: &str) -> PathBuf {
    let plain = dir.join("source.txt");
    fs::write(&plain, text).unwrap();
    let document = dir.join(format!("document.{extension}"));
    run(Command::new("/usr/bin/textutil")
        .args(["-convert", extension, "-output"])
        .arg(&document)
        .arg(&plain));
    document
}

/// Writes a workbook with two sheets.
fn write_workbook(dir: &Path) -> PathBuf {
    let parts = dir.join("workbook");
    let sheet = |label: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>{label}</t></is></c><c r="B1"><v>42</v></c></row>
</sheetData></worksheet>"#
        )
    };
    let files = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#
                .to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#
                .to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="First" sheetId="1" r:id="rId1"/><sheet name="Second" sheetId="2" r:id="rId2"/></sheets>
</workbook>"#
                .to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
</Relationships>"#
                .to_string(),
        ),
        ("xl/worksheets/sheet1.xml", sheet("alpha")),
        ("xl/worksheets/sheet2.xml", sheet("beta")),
    ];
    for (name, content) in files {
        let path = parts.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    let workbook = dir.join("workbook.xlsx");
    run(Command::new("/usr/bin/zip")
        .current_dir(&parts)
        .args(["-q", "-r", "-X"])
        .arg(&workbook)
        .arg("."));
    workbook
}

fn page_count(pdf: &Path) -> usize {
    block_on(PdfWorker::open(pdf.to_path_buf(), None))
        .unwrap()
        .pages
        .len()
}

#[test]
#[ignore = "runs Quick Look, textutil and a web view"]
fn converts_a_word_document() {
    let dir = tempfile::tempdir().unwrap();
    let document = write_document(
        dir.path(),
        "docx",
        "Hello from a document.\n\nSecond line.\n",
    );

    let pdf = convert_cached(
        &QuickLookConverter,
        &document,
        &dir.path().join("cache"),
        DEFAULT_TIMEOUT,
    )
    .unwrap();

    assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
    assert_eq!(page_count(&pdf), 1);
    // Nothing but the PDF is left in the cache.
    let leftovers: Vec<_> = fs::read_dir(pdf.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path != &pdf)
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
}

#[test]
#[ignore = "runs Quick Look, textutil and a web view"]
fn converts_rich_text_through_textutil() {
    let dir = tempfile::tempdir().unwrap();
    let document = write_document(dir.path(), "rtf", "Hello from rich text.\n");
    let pdf = dir.path().join("out.pdf");

    QuickLookConverter
        .convert(&document, &pdf, DEFAULT_TIMEOUT)
        .unwrap();

    assert_eq!(page_count(&pdf), 1);
}

#[test]
#[ignore = "runs Quick Look, textutil and a web view"]
fn converts_every_sheet_of_a_workbook() {
    let dir = tempfile::tempdir().unwrap();
    let workbook = write_workbook(dir.path());
    let pdf = dir.path().join("out.pdf");

    QuickLookConverter
        .convert(&workbook, &pdf, DEFAULT_TIMEOUT)
        .unwrap();

    assert_eq!(page_count(&pdf), 2);
}

#[test]
#[ignore = "runs Quick Look, textutil and a web view"]
fn a_file_quick_look_cannot_read_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("broken.xlsx");
    fs::write(&document, "this is not a workbook").unwrap();
    let pdf = dir.path().join("out.pdf");

    let result = QuickLookConverter.convert(&document, &pdf, DEFAULT_TIMEOUT);

    assert!(result.is_err(), "converted garbage: {result:?}");
    assert!(!dir.path().join("out.work").exists());
}

#[test]
#[ignore = "runs Quick Look, textutil and a web view"]
fn a_conversion_past_its_time_limit_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let document = write_document(dir.path(), "docx", "Hello.\n");
    let pdf = dir.path().join("out.pdf");

    let result = QuickLookConverter.convert(&document, &pdf, Duration::from_millis(1));

    assert_eq!(result, Err(ConvertError::TimedOut));
    assert!(!pdf.exists());
}
