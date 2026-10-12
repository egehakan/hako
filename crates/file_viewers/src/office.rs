//! Showing office documents by converting them to PDF, once per version of the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use thiserror::Error;

#[cfg(target_os = "macos")]
#[path = "office_mac.rs"]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::QuickLookConverter;

/// How long a conversion may take before it is given up on.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

/// Folder of the conversions inside the cache directory of the app.
const CACHE_FOLDER: &str = "document-previews";

/// Conversions kept in the cache; the oldest ones beyond this are deleted.
const MAX_CACHED_CONVERSIONS: usize = 64;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ConvertError {
    #[error("Documents of this kind cannot be shown on this platform.")]
    Unsupported,
    #[error("The document could not be read: {0}")]
    Io(String),
    #[error("Converting the document took too long.")]
    TimedOut,
    #[error("The document could not be converted: {0}")]
    Failed(String),
}

impl From<io::Error> for ConvertError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Turns an office document into a PDF.
///
/// The conversion leans on tools whose output is not documented, so it sits behind this trait
/// to be replaceable without touching the viewer.
pub trait OfficeConverter: Send + Sync {
    /// Writes a PDF showing `source` to `destination`, within `timeout`.
    fn convert(
        &self,
        source: &Path,
        destination: &Path,
        timeout: Duration,
    ) -> Result<(), ConvertError>;
}

/// The converter for this platform, if it has one.
pub fn system_converter() -> Option<Arc<dyn OfficeConverter>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(QuickLookConverter))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// One version of a file: converting it again gives the same result as long as this is equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVersion {
    path: PathBuf,
    modified_nanos: u128,
    size: u64,
}

impl FileVersion {
    pub fn of(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        Ok(Self {
            path: path.to_path_buf(),
            modified_nanos,
            size: metadata.len(),
        })
    }

    /// Name of the file that caches the conversion of this version.
    pub fn cache_file_name(&self) -> String {
        // FNV-1a, written out because the hash must stay the same across builds of the app,
        // which the hashers of the standard library do not promise.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(self.path.as_os_str().as_encoded_bytes());
        feed(&[0]);
        feed(&self.modified_nanos.to_le_bytes());
        feed(&self.size.to_le_bytes());
        format!("{hash:016x}.pdf")
    }
}

/// Where the conversion of `version` is cached under `cache_dir`.
pub fn cached_pdf_path(cache_dir: &Path, version: &FileVersion) -> PathBuf {
    cache_dir.join(CACHE_FOLDER).join(version.cache_file_name())
}

/// Returns a PDF showing `source`, converting it with `converter` unless this version of the
/// file was converted before.
pub fn convert_cached(
    converter: &dyn OfficeConverter,
    source: &Path,
    cache_dir: &Path,
    timeout: Duration,
) -> Result<PathBuf, ConvertError> {
    let version = FileVersion::of(source)?;
    let cached = cached_pdf_path(cache_dir, &version);
    if is_usable_pdf(&cached) {
        return Ok(cached);
    }

    let folder = cache_dir.join(CACHE_FOLDER);
    fs::create_dir_all(&folder)?;
    // Converted next to the final file and renamed into place, so that a conversion cut short
    // never leaves a truncated PDF under the cached name.
    let partial = cached.with_extension(format!("{}.partial", std::process::id()));
    let converted = converter.convert(source, &partial, timeout);
    if let Err(err) = converted {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    if !is_usable_pdf(&partial) {
        let _ = fs::remove_file(&partial);
        return Err(ConvertError::Failed(
            "the conversion produced no PDF".to_string(),
        ));
    }
    fs::rename(&partial, &cached)?;
    prune_cache(&folder, MAX_CACHED_CONVERSIONS);
    Ok(cached)
}

fn is_usable_pdf(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// Deletes all but the `keep` most recently written conversions in `folder`.
fn prune_cache(folder: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut conversions: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "pdf"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    conversions.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in conversions.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
#[path = "office_tests.rs"]
mod tests;
