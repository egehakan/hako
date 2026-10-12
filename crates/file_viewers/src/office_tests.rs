use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;

use super::*;

/// Writes a recognisable PDF stand-in and counts how often it was asked to.
#[derive(Default)]
struct CountingConverter {
    conversions: AtomicUsize,
}

impl OfficeConverter for CountingConverter {
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        _timeout: Duration,
    ) -> Result<(), ConvertError> {
        self.conversions.fetch_add(1, Ordering::SeqCst);
        let content = fs::read_to_string(source)?;
        fs::write(destination, format!("%PDF of {content}"))?;
        Ok(())
    }
}

struct FailingConverter {
    leave_partial_output: bool,
}

impl OfficeConverter for FailingConverter {
    fn convert(
        &self,
        _source: &Path,
        destination: &Path,
        _timeout: Duration,
    ) -> Result<(), ConvertError> {
        if self.leave_partial_output {
            fs::write(destination, "half a PDF")?;
        }
        Err(ConvertError::TimedOut)
    }
}

/// Reports success without writing anything.
struct SilentConverter;

impl OfficeConverter for SilentConverter {
    fn convert(&self, _: &Path, _: &Path, _: Duration) -> Result<(), ConvertError> {
        Ok(())
    }
}

fn cached_files(cache_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(cache_dir.join(CACHE_FOLDER)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_document_is_converted_once_per_version() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let source = dir.path().join("report.docx");
    fs::write(&source, "first").unwrap();
    let converter = CountingConverter::default();

    let first = convert_cached(&converter, &source, &cache, DEFAULT_TIMEOUT).unwrap();
    let again = convert_cached(&converter, &source, &cache, DEFAULT_TIMEOUT).unwrap();

    assert_eq!(first, again);
    assert_eq!(converter.conversions.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read_to_string(&first).unwrap(), "%PDF of first");
    assert!(first.starts_with(cache.join(CACHE_FOLDER)));
}

#[test]
fn a_changed_document_is_converted_again() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let source = dir.path().join("report.docx");
    fs::write(&source, "first").unwrap();
    let converter = CountingConverter::default();
    let first = convert_cached(&converter, &source, &cache, DEFAULT_TIMEOUT).unwrap();

    fs::write(&source, "second, and longer").unwrap();
    let second = convert_cached(&converter, &source, &cache, DEFAULT_TIMEOUT).unwrap();

    assert_ne!(first, second);
    assert_eq!(converter.conversions.load(Ordering::SeqCst), 2);
    assert_eq!(
        fs::read_to_string(&second).unwrap(),
        "%PDF of second, and longer"
    );
}

#[test]
fn the_cache_name_depends_on_path_time_and_size() {
    let version = FileVersion {
        path: PathBuf::from("/tmp/report.docx"),
        modified_nanos: 1_700_000_000_000_000_000,
        size: 4096,
    };
    let name = version.cache_file_name();
    assert_eq!(name, version.clone().cache_file_name());
    assert_eq!(name.len(), 20);
    assert!(name.ends_with(".pdf"));

    let moved = FileVersion {
        path: PathBuf::from("/tmp/report-copy.docx"),
        ..version.clone()
    };
    let touched = FileVersion {
        modified_nanos: version.modified_nanos + 1,
        ..version.clone()
    };
    let grown = FileVersion {
        size: 4097,
        ..version.clone()
    };
    for other in [moved, touched, grown] {
        assert_ne!(other.cache_file_name(), name);
    }
}

#[test]
fn a_failed_conversion_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let source = dir.path().join("report.docx");
    fs::write(&source, "content").unwrap();

    for leave_partial_output in [false, true] {
        let converter = FailingConverter {
            leave_partial_output,
        };
        let result = convert_cached(&converter, &source, &cache, DEFAULT_TIMEOUT);
        assert_eq!(result, Err(ConvertError::TimedOut));
        assert!(cached_files(&cache).is_empty());
    }
}

#[test]
fn a_conversion_that_writes_nothing_is_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("report.docx");
    fs::write(&source, "content").unwrap();

    let result = convert_cached(&SilentConverter, &source, dir.path(), DEFAULT_TIMEOUT);
    assert!(matches!(result, Err(ConvertError::Failed(_))));
    assert!(cached_files(dir.path()).is_empty());
}

#[test]
fn a_missing_document_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = convert_cached(
        &CountingConverter::default(),
        &dir.path().join("missing.docx"),
        dir.path(),
        DEFAULT_TIMEOUT,
    );
    assert!(matches!(result, Err(ConvertError::Io(_))));
}

#[test]
fn old_conversions_are_pruned() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        fs::write(dir.path().join(name), name).unwrap();
        // File times on some filesystems have a coarse resolution.
        sleep(Duration::from_millis(20));
    }
    fs::write(dir.path().join("unrelated.txt"), "kept").unwrap();

    prune_cache(dir.path(), 2);

    assert!(!dir.path().join("a.pdf").exists());
    assert!(dir.path().join("b.pdf").exists());
    assert!(dir.path().join("c.pdf").exists());
    assert!(dir.path().join("unrelated.txt").exists());
}

#[test]
fn only_macos_has_a_converter() {
    assert_eq!(system_converter().is_some(), cfg!(target_os = "macos"));
}
