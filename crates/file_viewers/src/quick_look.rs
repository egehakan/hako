//! The Quick Look panel of macOS, which previews any file the system has a generator for.

use std::io;
use std::path::Path;

/// Whether [`show`] can do anything on this platform.
pub const fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// Opens the Quick Look panel on the file at `path`. The panel belongs to a helper process of
/// the system and stays up until the user closes it.
#[cfg(target_os = "macos")]
pub fn show(path: &Path) -> io::Result<()> {
    use std::process::Stdio;
    use std::thread;

    use command::blocking::Command;

    let mut panel = Command::new("/usr/bin/qlmanage")
        .arg("-p")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reap the helper when the panel closes, or it lingers as a zombie process.
    thread::Builder::new()
        .name("quick-look".to_string())
        .spawn(move || {
            let _ = panel.wait();
        })?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn show(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Quick Look is only available on macOS",
    ))
}
