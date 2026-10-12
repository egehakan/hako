//! Office documents to PDF with the tools of macOS.
//!
//! Quick Look exports its preview of a document as HTML (`qlmanage -p -o`), and a web view
//! that never goes on screen prints that HTML to PDF. Both run in helper processes: nothing
//! touches the main thread of the app, and a step that hangs is killed at the time limit.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use command::blocking::Command;
use instant::Instant;

use super::{ConvertError, OfficeConverter};
use crate::process::{self, RunError};

const HELPER_SCRIPT: &str = include_str!("office_to_pdf.js");
const HELPER_SCRIPT_NAME: &str = "office_to_pdf.js";

/// Formats `textutil` reads. It is used for the ones Quick Look exports no HTML for, and as a
/// second try for the others.
const TEXTUTIL_EXTENSIONS: &[&str] = &["rtf", "odt", "doc", "docx"];

/// Formats whose Quick Look preview is not HTML, which go straight to `textutil`.
const TEXTUTIL_FIRST_EXTENSIONS: &[&str] = &["rtf", "odt"];

pub struct QuickLookConverter;

impl OfficeConverter for QuickLookConverter {
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        timeout: Duration,
    ) -> Result<(), ConvertError> {
        let deadline = Instant::now() + timeout;
        let work_dir = WorkDir::create(destination)?;
        match export_preview(source, work_dir.path(), deadline)? {
            Preview::Pdf(pdf) => {
                fs::copy(pdf, destination)?;
                Ok(())
            }
            Preview::Html(html) => html_to_pdf(&html, destination, work_dir.path(), deadline),
        }
    }
}

/// A scratch folder that is deleted when the conversion ends, whichever way it ends.
struct WorkDir(PathBuf);

impl WorkDir {
    fn create(destination: &Path) -> Result<Self, ConvertError> {
        let path = destination.with_extension("work");
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What a document was exported as.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Preview {
    Pdf(PathBuf),
    /// One HTML file, or the `.qlpreview` folder of a Quick Look export.
    Html(PathBuf),
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

fn export_preview(
    source: &Path,
    work_dir: &Path,
    deadline: Instant,
) -> Result<Preview, ConvertError> {
    if has_extension(source, TEXTUTIL_FIRST_EXTENSIONS) {
        return export_with_textutil(source, work_dir, deadline);
    }

    run_tool(
        Command::new("/usr/bin/qlmanage")
            .arg("-p")
            .arg("-o")
            .arg(work_dir)
            .arg(source),
        deadline,
    )?;
    if let Some(preview) = locate_preview(work_dir) {
        return Ok(preview);
    }
    if has_extension(source, TEXTUTIL_EXTENSIONS) {
        return export_with_textutil(source, work_dir, deadline);
    }
    Err(ConvertError::Failed(
        "Quick Look has no preview for this file".to_string(),
    ))
}

fn export_with_textutil(
    source: &Path,
    work_dir: &Path,
    deadline: Instant,
) -> Result<Preview, ConvertError> {
    let html = work_dir.join("Preview.html");
    let finished = run_tool(
        Command::new("/usr/bin/textutil")
            .arg("-convert")
            .arg("html")
            .arg("-output")
            .arg(&html)
            .arg(source),
        deadline,
    )?;
    if !finished.success || !html.is_file() {
        return Err(ConvertError::Failed(
            "the document could not be read".to_string(),
        ));
    }
    Ok(Preview::Html(html))
}

/// Finds what `qlmanage` exported into `work_dir`: a folder named after the document with a
/// `Preview.pdf` or a `Preview.html` in it.
pub(super) fn locate_preview(work_dir: &Path) -> Option<Preview> {
    let export = fs::read_dir(work_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && has_extension(path, &["qlpreview"]))?;
    let pdf = export.join("Preview.pdf");
    if pdf.is_file() {
        return Some(Preview::Pdf(pdf));
    }
    export
        .join("Preview.html")
        .is_file()
        .then_some(Preview::Html(export))
}

fn html_to_pdf(
    html: &Path,
    destination: &Path,
    work_dir: &Path,
    deadline: Instant,
) -> Result<(), ConvertError> {
    let script = work_dir.join(HELPER_SCRIPT_NAME);
    fs::write(&script, HELPER_SCRIPT)?;
    let seconds_left = deadline
        .saturating_duration_since(Instant::now())
        .as_secs_f32();
    let finished = run_tool(
        Command::new("/usr/bin/osascript")
            .arg("-l")
            .arg("JavaScript")
            .arg(&script)
            .arg(html)
            .arg(destination)
            .arg(format!("{seconds_left:.1}")),
        deadline,
    )?;

    let report = finished.stdout.trim();
    if finished.success && report == "ok" {
        return Ok(());
    }
    let reason = report.strip_prefix("error: ").unwrap_or(report);
    Err(ConvertError::Failed(if reason.is_empty() {
        "the PDF export failed".to_string()
    } else {
        reason.to_string()
    }))
}

fn run_tool(command: &mut Command, deadline: Instant) -> Result<process::Finished, ConvertError> {
    process::run(command, deadline).map_err(|err| match err {
        RunError::Io(err) => ConvertError::Io(err.to_string()),
        RunError::TimedOut => ConvertError::TimedOut,
    })
}

#[cfg(test)]
#[path = "office_mac_tests.rs"]
mod tests;
