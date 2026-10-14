use serde_json::json;

use super::*;
use crate::protocol::ErrorCode;

#[test]
fn frame_header_layout() {
    let header = FrameHeader {
        bid: 0x0102_0304,
        frame: 7,
        css_width: 393,
        css_height: 852,
        format: ImageFormat::Jpeg,
    };
    let frame = encode_browser_frame(header, b"img");
    assert_eq!(
        frame,
        [
            0x03, 0x01, 0x02, 0x03, 0x04, 0, 0, 0, 7, 0x01, 0x89, 0x03, 0x54, 1, b'i', b'm', b'g'
        ]
    );
    assert_eq!(frame.len(), BROWSER_FRAME_HEADER_BYTES + 3);
}

#[test]
fn viewport_defaults_and_limits() {
    let viewport: Viewport =
        serde_json::from_value(json!({ "width": 393, "height": 852 })).unwrap();
    assert_eq!(viewport.scale, 1.0);
    assert!(!viewport.mobile && !viewport.touch && viewport.user_agent.is_none());
    assert!(viewport.validate().is_ok());

    let phone = Viewport {
        scale: 3.0,
        ..viewport.clone()
    };
    assert_eq!(phone.image_size(), (1179, 2556));
    assert!(!phone.landscape());

    for bad in [
        Viewport {
            width: 0,
            ..viewport.clone()
        },
        Viewport {
            height: 5000,
            ..viewport.clone()
        },
        Viewport {
            scale: 0.5,
            ..viewport.clone()
        },
        Viewport {
            scale: 3.5,
            ..viewport
        },
    ] {
        assert_eq!(bad.validate().unwrap_err().code, ErrorCode::BadRequest);
    }
}

#[test]
fn local_urls() {
    for url in [
        "http://localhost:3000/",
        "http://LOCALHOST:3000/a",
        "http://myapp.localhost:8790/",
        "http://127.0.0.1:8000/",
        "http://[::1]:5173/",
        "http://0.0.0.0:4000/",
        "https://localhost/",
    ] {
        assert!(is_local_url(url), "{url}");
    }
    for url in [
        "https://accounts.google.com/",
        "http://localhost.example.com/",
        "http://192.168.1.2:3000/",
        "about:blank",
        "file:///etc/hosts",
        "not a url",
    ] {
        assert!(!is_local_url(url), "{url}");
    }
}

#[test]
fn resolving_typed_addresses() {
    let ok = |input: &str| resolve_url(input).unwrap();
    assert_eq!(ok("localhost:3000"), "http://localhost:3000/");
    assert_eq!(ok("127.0.0.1:8000/admin"), "http://127.0.0.1:8000/admin");
    assert_eq!(ok("example.com"), "https://example.com/");
    assert_eq!(ok("http://example.com/x?y=1"), "http://example.com/x?y=1");
    assert_eq!(
        ok(" https://accounts.google.com/ "),
        "https://accounts.google.com/"
    );
    assert_eq!(ok("about:blank"), "about:blank");
    for bad in [
        "?",
        "",
        "javascript:alert(1)",
        "file:///etc/hosts",
        "data:text/html,hi",
        "chrome://settings",
    ] {
        assert_eq!(
            resolve_url(bad).unwrap_err().code,
            ErrorCode::BadRequest,
            "{bad}"
        );
    }
}

#[test]
fn preview_addresses() {
    let address = PreviewAddress {
        port: 5173,
        url_path: "/app".to_owned(),
    };
    assert_eq!(
        preview_url(&address, None).unwrap(),
        "http://localhost:5173/app"
    );
    assert_eq!(
        preview_url(&address, Some("/login?next=%2F")).unwrap(),
        "http://localhost:5173/login?next=%2F"
    );
    assert_eq!(
        preview_url(&address, Some("localhost:8087/health")).unwrap(),
        "http://localhost:8087/health"
    );
}

#[test]
fn method_names() {
    assert_eq!(
        BrowserMethod::from_name("browser.open"),
        Some(BrowserMethod::Open)
    );
    assert_eq!(
        BrowserMethod::from_name("browser.file_chooser_reply"),
        Some(BrowserMethod::FileChooserReply)
    );
    assert_eq!(
        BrowserMethod::from_name("browser.ack"),
        Some(BrowserMethod::Ack)
    );
    assert_eq!(BrowserMethod::from_name("browser.nope"), None);
}

#[test]
fn picker_and_dialog_shapes() {
    let picker: PagePicker = serde_json::from_value(json!({
        "kind": "select",
        "id": "p3",
        "multiple": false,
        "options": [
            { "label": "Apple", "value": "apple", "selected": false, "disabled": false },
            { "label": "Pear", "value": "pear", "selected": true, "disabled": false, "group": "Fruit" },
        ],
    }))
    .unwrap();
    let event = PickerEvent::new(4, "T1".to_owned(), "9".to_owned(), picker);
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({
            "bid": 4, "tab": "T1", "kind": "select", "id": "9", "multiple": false,
            "options": [
                { "label": "Apple", "value": "apple", "selected": false, "disabled": false, "group": null },
                { "label": "Pear", "value": "pear", "selected": true, "disabled": false, "group": "Fruit" },
            ],
        })
    );

    let date: PagePicker = serde_json::from_value(json!({
        "kind": "datetime-local", "id": "p1", "value": "2026-10-14T09:30", "min": null,
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(PickerEvent::new(1, "T".to_owned(), "2".to_owned(), date)).unwrap(),
        json!({ "bid": 1, "tab": "T", "kind": "datetime-local", "id": "2", "multiple": false, "value": "2026-10-14T09:30" })
    );

    let dialog = DialogEvent {
        bid: 1,
        tab: "T".to_owned(),
        kind: DialogKind::from_devtools("prompt").unwrap(),
        message: "City?".to_owned(),
        default: "Ankara".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(dialog).unwrap(),
        json!({ "bid": 1, "tab": "T", "kind": "prompt", "message": "City?", "default": "Ankara" })
    );
    assert_eq!(
        DialogKind::from_devtools("beforeunload"),
        Some(DialogKind::Beforeunload)
    );

    let reply: DialogReplyParams =
        serde_json::from_value(json!({ "bid": 1, "accept": true, "text": "Izmir" })).unwrap();
    assert_eq!(reply.tab, None);
}

#[test]
fn console_helpers() {
    assert_eq!(
        ConsoleLevel::from_console_call("warning"),
        ConsoleLevel::Warn
    );
    assert_eq!(
        ConsoleLevel::from_console_call("assert"),
        ConsoleLevel::Error
    );
    assert_eq!(ConsoleLevel::from_console_call("table"), ConsoleLevel::Log);
    assert_eq!(
        network_status(
            "Failed to load resource: the server responded with a status of 404 (Not Found)"
        ),
        Some(404)
    );
    assert_eq!(
        network_status("Failed to load resource: net::ERR_CONNECTION_REFUSED"),
        None
    );
    assert_eq!(truncate_chars("ağaç", 2), "ağ");
    assert_eq!(truncate_chars("ab", 5), "ab");
}

#[test]
fn download_file_names() {
    assert_eq!(safe_file_name("report.pdf"), "report.pdf");
    assert_eq!(safe_file_name("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(safe_file_name(".hidden"), "hidden");
    assert_eq!(safe_file_name("  "), "download");
}
