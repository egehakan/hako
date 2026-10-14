use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;

#[test]
fn chrome_is_looked_for_in_order() {
    let home = Path::new("/Users/me");
    let installed = |apps: &'static [&'static str]| {
        move |path: &Path| apps.iter().any(|app| path == Path::new(app))
    };
    assert_eq!(
        find_app(
            home,
            installed(&[
                "/Users/me/Applications/Google Chrome.app",
                "/Applications/Google Chrome.app"
            ])
        ),
        Some(PathBuf::from("/Applications/Google Chrome.app"))
    );
    assert_eq!(
        find_app(
            home,
            installed(&[
                "/Users/me/Applications/Google Chrome.app",
                "/Applications/Chromium.app"
            ])
        ),
        Some(PathBuf::from("/Users/me/Applications/Google Chrome.app"))
    );
    assert_eq!(
        find_app(home, installed(&["/Users/me/Applications/Chromium.app"])),
        Some(PathBuf::from("/Users/me/Applications/Chromium.app"))
    );
    assert_eq!(find_app(home, installed(&[])), None);
}

#[test]
fn launched_without_automation_flags_on_a_fixed_port() {
    let arguments = arguments(Path::new("/Users/me/.hako/preview-browser"), 39222);
    assert!(arguments.contains(&"--user-data-dir=/Users/me/.hako/preview-browser".to_owned()));
    assert!(arguments.contains(&"--remote-debugging-port=39222".to_owned()));
    for flag in [
        "--enable-automation",
        "--headless",
        "--remote-allow-origins",
        "port=0",
    ] {
        assert!(
            !arguments.iter().any(|argument| argument.contains(flag)),
            "{flag}"
        );
    }
}

#[test]
fn the_browser_process_of_the_profile_is_found() {
    let profile = Path::new("/Users/me/.hako/preview-browser");
    let ps = "\
  101 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --no-startup-window
  202 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/me/.hako/preview-browser-old --remote-debugging-port=1
  303 /Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper --type=renderer --user-data-dir=/Users/me/.hako/preview-browser
  404 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/me/.hako/preview-browser --remote-debugging-port=39223 --no-first-run
";
    assert_eq!(find_process(ps, profile), Some((404, Some(39223))));
    assert_eq!(
        find_process(ps, Path::new("/Users/other/.hako/preview-browser")),
        None
    );
    let without_port = "  505 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=/Users/me/.hako/preview-browser";
    assert_eq!(find_process(without_port, profile), Some((505, None)));
}

#[test]
fn old_downloads_are_removed() {
    let directory = tempfile::tempdir().unwrap();
    let download = directory.path().join("guid-1");
    std::fs::create_dir(&download).unwrap();
    std::fs::write(download.join("report.pdf"), b"pdf").unwrap();
    let now = SystemTime::now();
    clean_downloads(directory.path(), now);
    assert!(download.exists());
    clean_downloads(
        directory.path(),
        now + DOWNLOADS_KEPT_FOR + Duration::from_secs(60),
    );
    assert!(!download.exists());
}

#[test]
fn a_response_ends_where_its_length_says() {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\n\r\n";
    assert!(!response_complete(head.as_bytes()));
    assert!(!response_complete(format!("{head}{{}}").as_bytes()));
    assert!(response_complete(format!("{head}{{  }}").as_bytes()));
    assert!(!response_complete(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n"
    ));
}
