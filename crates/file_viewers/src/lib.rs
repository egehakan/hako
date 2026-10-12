//! Logic behind the in-app file viewers: which viewer a file gets, decoding and rasterising its
//! content, and the layout maths of zooming and scrolling. Nothing here depends on the UI
//! framework, so all of it can be tested without the app.

use std::thread;

use futures::channel::oneshot;

#[cfg(target_os = "macos")]
mod bitmap;
pub mod cache;
pub mod csv;
pub mod geometry;
pub mod image;
#[cfg(target_os = "macos")]
mod imageio;
pub mod kind;
pub mod office;
pub mod pdf;
#[cfg(target_os = "macos")]
mod process;
pub mod quick_look;
pub mod scene;
pub mod svg;

/// Runs `work` on its own thread and resolves to its result, or to `None` if it panicked.
///
/// Decoding and conversion block for as long as the file takes, which would stall the shared
/// background executor if they ran on one of its threads.
pub fn run_blocking<T, F>(work: F) -> impl Future<Output = Option<T>> + Send + 'static
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    let spawned = thread::Builder::new()
        .name("file-viewer".to_string())
        .spawn(move || {
            let _ = sender.send(work());
        });
    if let Err(err) = spawned {
        log::warn!("Could not start a file viewer thread: {err}");
    }
    async move { receiver.await.ok() }
}

/// Formats a byte count the way a file manager does, e.g. `1.4 MB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    if value >= 100. {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
