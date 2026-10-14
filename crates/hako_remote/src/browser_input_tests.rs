use std::time::{Duration, Instant};

use serde_json::json;

use super::*;

fn event(value: Value) -> InputEvent {
    serde_json::from_value(value).unwrap()
}

fn types(calls: &[Call]) -> Vec<(&'static str, String)> {
    calls
        .iter()
        .map(|call| {
            (
                call.method,
                call.params["type"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[test]
fn key_codes() {
    let cases = [
        ("Enter", None, 13, Some("\r"), "Enter"),
        ("Backspace", None, 8, None, "Backspace"),
        ("Tab", None, 9, None, "Tab"),
        ("Escape", None, 27, None, "Escape"),
        ("ArrowLeft", None, 37, None, "ArrowLeft"),
        ("ArrowDown", None, 40, None, "ArrowDown"),
        ("Home", None, 36, None, "Home"),
        ("End", None, 35, None, "End"),
        ("PageUp", None, 33, None, "PageUp"),
        ("PageDown", None, 34, None, "PageDown"),
        ("Delete", None, 46, None, "Delete"),
        ("F5", None, 116, None, "F5"),
        (" ", None, 32, Some(" "), "Space"),
        ("a", None, 65, Some("a"), "KeyA"),
        ("A", Some("KeyA"), 65, Some("A"), "KeyA"),
        ("7", None, 55, Some("7"), "Digit7"),
        ("?", None, 191, Some("?"), "Slash"),
        ("ğ", None, 0, Some("ğ"), ""),
    ];
    for (key, code, key_code, text, expected_code) in cases {
        let info = key_info(key, code);
        assert_eq!(info.key_code, key_code, "{key}");
        assert_eq!(info.text.as_deref(), text, "{key}");
        assert_eq!(info.code, expected_code, "{key}");
    }
    assert_eq!(key_info("Shift", None).location, 1);
}

#[test]
fn modifiers_in_every_shape() {
    for value in [
        json!(12),
        json!(["meta", "shift"]),
        json!({ "meta": true, "shift": true, "alt": false }),
        json!({ "metaKey": true, "shiftKey": true }),
    ] {
        let modifiers: Modifiers = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            modifiers,
            Modifiers(Modifiers::META | Modifiers::SHIFT),
            "{value}"
        );
    }
    assert_eq!(
        serde_json::from_value::<Modifiers>(json!(null)).unwrap(),
        Modifiers(0)
    );
}

#[test]
fn a_key_press_is_down_then_up() {
    let mut input = InputTranslator::default();
    let calls = input.translate(
        event(json!({ "type": "key", "key": "Enter" })),
        false,
        Instant::now(),
    );
    assert_eq!(
        types(&calls),
        [
            ("Input.dispatchKeyEvent", "keyDown".to_owned()),
            ("Input.dispatchKeyEvent", "keyUp".to_owned())
        ]
    );
    assert_eq!(calls[0].params["text"], "\r");
    assert_eq!(calls[0].params["windowsVirtualKeyCode"], 13);

    let calls = input.translate(
        event(json!({ "type": "key", "key": "ArrowUp", "action": "down" })),
        false,
        Instant::now(),
    );
    assert_eq!(
        types(&calls),
        [("Input.dispatchKeyEvent", "rawKeyDown".to_owned())]
    );
    assert!(calls[0].params.get("text").is_none());
}

#[test]
fn mac_shortcuts_carry_editing_commands() {
    let mut input = InputTranslator::default();
    let mut shortcut = |key: &str, modifiers: Value| {
        let calls = input.translate(
            event(json!({ "type": "key", "key": key, "modifiers": modifiers })),
            false,
            Instant::now(),
        );
        (
            calls[0].params.get("commands").cloned(),
            calls[0].params.get("text").cloned(),
        )
    };
    assert_eq!(
        shortcut("a", json!(["meta"])),
        (Some(json!(["selectAll"])), None)
    );
    assert_eq!(
        shortcut("c", json!(["meta"])),
        (Some(json!(["copy"])), None)
    );
    assert_eq!(
        shortcut("v", json!(["meta"])),
        (Some(json!(["paste"])), None)
    );
    assert_eq!(
        shortcut("z", json!(["meta"])),
        (Some(json!(["undo"])), None)
    );
    assert_eq!(
        shortcut("Z", json!(["meta", "shift"])),
        (Some(json!(["redo"])), None)
    );
    assert_eq!(
        shortcut("ArrowLeft", json!(["meta", "shift"])).0,
        Some(json!(["moveToLeftEndOfLineAndModifySelection"]))
    );
    assert_eq!(
        shortcut("Backspace", json!(["alt"])).0,
        Some(json!(["deleteWordBackward"]))
    );
    // Ctrl+A is not a Mac shortcut for select all, and types nothing.
    assert_eq!(shortcut("a", json!(["ctrl"])), (None, None));
    assert_eq!(shortcut("A", json!(["shift"])), (None, Some(json!("A"))));
}

#[test]
fn touches_on_a_touch_page() {
    let mut input = InputTranslator::default();
    let now = Instant::now();
    let mut touch = |phase: &str, points: Value| {
        input.translate(
            event(json!({ "type": "touch", "phase": phase, "points": points })),
            true,
            now,
        )
    };
    let start = touch("start", json!([{ "x": 10, "y": 20, "id": 1 }]));
    assert_eq!(start[0].params["type"], "touchStart");
    assert_eq!(
        start[0].params["touchPoints"],
        json!([{ "x": 10.0, "y": 20.0, "id": 1 }])
    );

    let second = touch("start", json!([{ "x": 50, "y": 60, "id": 2 }]));
    assert_eq!(second[0].params["touchPoints"].as_array().unwrap().len(), 2);

    let moved = touch("move", json!([{ "x": 12, "y": 25, "id": 1 }]));
    assert_eq!(moved[0].params["type"], "touchMove");
    assert_eq!(moved[0].params["touchPoints"][0]["y"], 25.0);

    // One finger lifts: the other is still down.
    let lifted = touch("end", json!([{ "x": 12, "y": 25, "id": 1 }]));
    assert_eq!(lifted[0].params["type"], "touchMove");
    assert_eq!(
        lifted[0].params["touchPoints"],
        json!([{ "x": 50.0, "y": 60.0, "id": 2 }])
    );

    let ended = touch("end", json!([{ "x": 50, "y": 60, "id": 2 }]));
    assert_eq!(
        ended[0].params,
        json!({ "type": "touchEnd", "touchPoints": [] })
    );

    touch("start", json!([{ "x": 1, "y": 1, "id": 3 }]));
    let cancelled = touch("cancel", json!([]));
    assert_eq!(cancelled[0].params["type"], "touchCancel");
    assert!(input.touches.is_empty());
}

#[test]
fn touches_become_clicks_on_a_desktop_page() {
    let mut input = InputTranslator::default();
    let start = Instant::now();
    let mut touch = |phase: &str, x: f64, y: f64, at: Duration| {
        input.translate(
            event(
                json!({ "type": "touch", "phase": phase, "points": [{ "x": x, "y": y, "id": 0 }] }),
            ),
            false,
            start + at,
        )
    };
    let down = touch("start", 100.0, 200.0, Duration::ZERO);
    assert_eq!(
        types(&down),
        [
            ("Input.dispatchMouseEvent", "mouseMoved".to_owned()),
            ("Input.dispatchMouseEvent", "mousePressed".to_owned())
        ]
    );
    assert_eq!(down[1].params["button"], "left");
    assert_eq!(down[1].params["clickCount"], 1);
    let up = touch("end", 100.0, 200.0, Duration::from_millis(80));
    assert_eq!(up[0].params["type"], "mouseReleased");
    assert_eq!(up[0].params["clickCount"], 1);

    // A second tap close by and soon after is a double click.
    let down = touch("start", 103.0, 202.0, Duration::from_millis(250));
    assert_eq!(down[1].params["clickCount"], 2);
    let up = touch("end", 103.0, 202.0, Duration::from_millis(300));
    assert_eq!(up[0].params["clickCount"], 2);

    // Too late for a triple.
    let down = touch("start", 103.0, 202.0, Duration::from_millis(1200));
    assert_eq!(down[1].params["clickCount"], 1);
    let dragged = touch("move", 150.0, 260.0, Duration::from_millis(1300));
    assert_eq!(dragged[0].params["buttons"], 1);
    let cancelled = touch("cancel", 150.0, 260.0, Duration::from_millis(1400));
    assert_eq!(cancelled[0].params["x"], -1.0);
    assert!(touch("move", 1.0, 1.0, Duration::from_millis(1500)).is_empty());
}

#[test]
fn mouse_scroll_and_text() {
    let mut input = InputTranslator::default();
    let now = Instant::now();
    let down = input.translate(
        event(
            json!({ "type": "mouse", "action": "down", "x": 5, "y": 6, "button": 2, "clicks": 1 }),
        ),
        false,
        now,
    );
    assert_eq!(down[0].params["button"], "right");
    assert_eq!(down[0].params["buttons"], 2);
    let moved = input.translate(
        event(json!({ "type": "mouse", "action": "move", "x": 7, "y": 8 })),
        false,
        now,
    );
    assert_eq!(moved[0].params["button"], "right");
    let up = input.translate(
        event(json!({ "type": "mouse", "action": "up", "x": 7, "y": 8 })),
        false,
        now,
    );
    assert_eq!(up[0].params["button"], "right");

    let wheel = input.translate(
        event(json!({ "type": "scroll", "x": 100, "y": 300, "dx": 0, "dy": 240 })),
        true,
        now,
    );
    assert_eq!(
        wheel[0].params,
        json!({ "type": "mouseWheel", "x": 100.0, "y": 300.0, "deltaX": 0.0, "deltaY": 240.0, "modifiers": 0 })
    );

    let text = input.translate(
        event(json!({ "type": "text", "text": "Merhaba dünya" })),
        false,
        now,
    );
    assert_eq!(
        text,
        [Call::new(
            "Input.insertText",
            json!({ "text": "Merhaba dünya" })
        )]
    );
    assert!(
        input
            .translate(event(json!({ "type": "text", "text": "" })), false, now)
            .is_empty()
    );
}
