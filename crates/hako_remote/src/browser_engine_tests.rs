use serde_json::json;

use super::*;

#[test]
fn a_rotated_phone_is_a_landscape_touch_screen() {
    let viewport = Viewport {
        width: 852,
        height: 393,
        scale: 3.0,
        mobile: true,
        touch: true,
        user_agent: Some("iPhone".to_owned()),
    };
    let calls = emulation(&viewport, "Chrome");
    assert_eq!(
        calls,
        [
            (
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": 852, "height": 393, "deviceScaleFactor": 3.0, "mobile": true,
                    "screenOrientation": { "type": "landscapePrimary", "angle": 90 },
                })
            ),
            (
                "Emulation.setTouchEmulationEnabled",
                json!({ "enabled": true, "maxTouchPoints": 5 })
            ),
            (
                "Emulation.setUserAgentOverride",
                json!({ "userAgent": "iPhone" })
            ),
        ]
    );

    // A desktop viewport goes back to the browser's own user agent.
    let desktop = Viewport {
        width: 1440,
        height: 900,
        scale: 1.0,
        mobile: false,
        touch: false,
        user_agent: None,
    };
    let calls = emulation(&desktop, "Chrome/155");
    assert_eq!(calls[1].1, json!({ "enabled": false }));
    assert_eq!(calls[2].1, json!({ "userAgent": "Chrome/155" }));
}

#[test]
fn attributes_and_console_arguments() {
    let attributes = json!(["type", "file", "accept", "image/*,.txt", "multiple", ""]);
    assert_eq!(
        attribute(&attributes, "accept").as_deref(),
        Some("image/*,.txt")
    );
    assert_eq!(attribute(&attributes, "capture"), None);

    assert_eq!(
        describe(&json!({ "type": "string", "value": "load error" })),
        "load error"
    );
    assert_eq!(describe(&json!({ "type": "number", "value": 42 })), "42");
    assert_eq!(
        describe(
            &json!({ "type": "object", "subtype": "error", "description": "TypeError: x is undefined" })
        ),
        "TypeError: x is undefined"
    );
    assert_eq!(
        describe(&json!({ "type": "number", "unserializableValue": "NaN" })),
        "NaN"
    );
    assert_eq!(describe(&json!({ "type": "undefined" })), "undefined");
}

#[test]
fn history_leads_back_but_not_to_the_blank_start() {
    let history = |current: u64, urls: &[&str]| {
        let entries: Vec<Value> = urls
            .iter()
            .enumerate()
            .map(|(index, url)| json!({ "id": 10 + index, "url": url, "title": format!("Page {index}") }))
            .collect();
        HistoryMoves::of(&json!({ "currentIndex": current, "entries": entries }))
    };
    let first = history(1, &["about:blank", "http://localhost:3000/"]);
    assert_eq!((first.back, first.forward), (None, None));
    assert_eq!(first.title.as_deref(), Some("Page 1"));

    let middle = history(
        2,
        &[
            "about:blank",
            "http://localhost:3000/",
            "http://localhost:3000/a",
            "http://localhost:3000/b",
        ],
    );
    assert_eq!((middle.back, middle.forward), (Some(11), Some(13)));

    let pop_up = history(
        1,
        &[
            "http://localhost:3000/popup",
            "https://accounts.google.com/",
        ],
    );
    assert_eq!(pop_up.back, Some(10));
}
