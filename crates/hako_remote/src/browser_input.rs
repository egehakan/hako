//! Input from a remote client, turned into the DevTools protocol's `Input` commands: touches
//! (passed on as touches, or as a left mouse button where the page is not shown as a touch
//! device), mouse, scrolling, text and keys with the key codes and macOS editing commands that a
//! real keyboard would come with.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};

/// Presses closer than this in time and space count as one double click.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 8.0;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    Touch {
        phase: TouchPhase,
        points: Vec<TouchPoint>,
    },
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        /// For a move, the button held during it; left out, the one pressed last is still held.
        #[serde(default)]
        button: Option<MouseButton>,
        /// The click count of a press or release.
        #[serde(default)]
        clicks: Option<u32>,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default)]
        modifiers: Modifiers,
    },
    /// A wheel turn at a point: positive `dy` scrolls the page down.
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Text {
        text: String,
    },
    Key {
        /// As `KeyboardEvent.key` names it, such as `a`, `A`, `Enter` or `ArrowLeft`.
        key: String,
        /// As `KeyboardEvent.code` names it, such as `KeyA`.
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        modifiers: Modifiers,
        #[serde(default)]
        action: KeyAction,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct TouchPoint {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Move,
    Down,
    Up,
    Wheel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseButton {
    #[default]
    None,
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }

    /// The bit of `MouseEvent.buttons` it sets.
    fn bit(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
        }
    }
}

/// Accepts `MouseEvent.button` numbers (0 left, 1 middle, 2 right) or names.
impl<'de> Deserialize<'de> for MouseButton {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Null => Ok(Self::None),
            Value::Number(number) => match number.as_i64() {
                Some(0) => Ok(Self::Left),
                Some(1) => Ok(Self::Middle),
                Some(2) => Ok(Self::Right),
                Some(-1) => Ok(Self::None),
                _ => Err(de::Error::custom("unknown mouse button")),
            },
            Value::String(name) => match name.as_str() {
                "left" => Ok(Self::Left),
                "middle" => Ok(Self::Middle),
                "right" => Ok(Self::Right),
                "none" => Ok(Self::None),
                _ => Err(de::Error::custom("unknown mouse button")),
            },
            Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
                Err(de::Error::custom("unknown mouse button"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    /// Down and up.
    #[default]
    Press,
    Down,
    Up,
}

/// Modifier keys held, as the DevTools protocol's bit mask: Alt 1, Ctrl 2, Meta (Cmd) 4, Shift 8.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const ALT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const META: u8 = 4;
    pub const SHIFT: u8 = 8;

    fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    fn bit_of(name: &str) -> Option<u8> {
        match name.to_ascii_lowercase().as_str() {
            "alt" | "option" => Some(Self::ALT),
            "ctrl" | "control" => Some(Self::CTRL),
            "meta" | "cmd" | "command" => Some(Self::META),
            "shift" => Some(Self::SHIFT),
            _ => None,
        }
    }
}

/// Accepts the bit mask, a list of names (`["meta", "shift"]`) or an object of flags
/// (`{ "meta": true }`, also `metaKey` and the like).
impl<'de> Deserialize<'de> for Modifiers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModifiersVisitor;

        impl<'de> Visitor<'de> for ModifiersVisitor {
            type Value = Modifiers;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a modifier bit mask, list or object")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Modifiers, E> {
                Ok(Modifiers((value & 0x0f) as u8))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Modifiers, E> {
                Ok(Modifiers((value & 0x0f) as u8))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Modifiers, E> {
                Ok(Modifiers::default())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Modifiers, A::Error> {
                let mut bits = 0;
                while let Some(name) = seq.next_element::<String>()? {
                    bits |= Modifiers::bit_of(&name).unwrap_or(0);
                }
                Ok(Modifiers(bits))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Modifiers, A::Error> {
                let mut bits = 0;
                while let Some((name, on)) = map.next_entry::<String, Value>()? {
                    let name = name.strip_suffix("Key").unwrap_or(&name);
                    if on.as_bool() == Some(true) {
                        bits |= Modifiers::bit_of(name).unwrap_or(0);
                    }
                }
                Ok(Modifiers(bits))
            }
        }

        deserializer.deserialize_any(ModifiersVisitor)
    }
}

/// One command of the DevTools protocol.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub params: Value,
}

impl Call {
    fn new(method: &'static str, params: Value) -> Self {
        Self { method, params }
    }
}

/// Turns a client's input into commands, keeping what a sequence of events needs: the fingers
/// down, the button held and the last click, to count double clicks.
#[derive(Debug, Default)]
pub struct InputTranslator {
    touches: Vec<TouchPoint>,
    /// The finger a touch translated to the mouse follows.
    mouse_finger: Option<u32>,
    held: MouseButton,
    last_press: Option<Press>,
    last_position: (f64, f64),
}

#[derive(Clone, Copy, Debug)]
struct Press {
    at: Instant,
    x: f64,
    y: f64,
    count: u32,
}

impl InputTranslator {
    /// The commands for `event`. `touch` says whether the page is shown as a touch device.
    pub fn translate(&mut self, event: InputEvent, touch: bool, now: Instant) -> Vec<Call> {
        match event {
            InputEvent::Touch { phase, points } if touch => self.touch(phase, &points),
            InputEvent::Touch { phase, points } => self.touch_as_mouse(phase, &points, now),
            InputEvent::Mouse {
                action,
                x,
                y,
                button,
                clicks,
                dx,
                dy,
                modifiers,
            } => self.mouse(action, x, y, button, clicks, (dx, dy), modifiers),
            InputEvent::Scroll { x, y, dx, dy } => vec![wheel(x, y, dx, dy, Modifiers::default())],
            InputEvent::Text { text } if text.is_empty() => Vec::new(),
            InputEvent::Text { text } => {
                vec![Call::new("Input.insertText", json!({ "text": text }))]
            }
            InputEvent::Key {
                key,
                code,
                modifiers,
                action,
            } => key_calls(&key, code.as_deref(), modifiers, action),
        }
    }

    fn touch(&mut self, phase: TouchPhase, points: &[TouchPoint]) -> Vec<Call> {
        match phase {
            TouchPhase::Start | TouchPhase::Move => {
                for point in points {
                    match self.touches.iter_mut().find(|touch| touch.id == point.id) {
                        Some(touch) => *touch = *point,
                        None => self.touches.push(*point),
                    }
                }
                if self.touches.is_empty() {
                    return Vec::new();
                }
                let kind = match phase {
                    TouchPhase::Start => "touchStart",
                    TouchPhase::Move | TouchPhase::End | TouchPhase::Cancel => "touchMove",
                };
                vec![touch_call(kind, &self.touches)]
            }
            TouchPhase::End => {
                if points.is_empty() {
                    self.touches.clear();
                } else {
                    self.touches
                        .retain(|touch| !points.iter().any(|point| point.id == touch.id));
                }
                if self.touches.is_empty() {
                    vec![touch_call("touchEnd", &[])]
                } else {
                    // The points no longer listed are the ones released.
                    vec![touch_call("touchMove", &self.touches)]
                }
            }
            TouchPhase::Cancel => {
                self.touches.clear();
                vec![touch_call("touchCancel", &[])]
            }
        }
    }

    fn touch_as_mouse(
        &mut self,
        phase: TouchPhase,
        points: &[TouchPoint],
        now: Instant,
    ) -> Vec<Call> {
        let point = match self.mouse_finger {
            Some(finger) => points.iter().find(|point| point.id == finger),
            None => points.first(),
        };
        match phase {
            TouchPhase::Start => {
                let Some(point) = point.copied() else {
                    return Vec::new();
                };
                if self.mouse_finger.is_some() {
                    return Vec::new();
                }
                self.mouse_finger = Some(point.id);
                let count = self.click_count(point.x, point.y, now);
                self.held = MouseButton::Left;
                self.last_position = (point.x, point.y);
                vec![
                    mouse_call(
                        "mouseMoved",
                        point.x,
                        point.y,
                        MouseButton::None,
                        0,
                        0,
                        Modifiers::default(),
                    ),
                    mouse_call(
                        "mousePressed",
                        point.x,
                        point.y,
                        MouseButton::Left,
                        1,
                        count,
                        Modifiers::default(),
                    ),
                ]
            }
            TouchPhase::Move => {
                let Some(point) = point.copied().filter(|_| self.mouse_finger.is_some()) else {
                    return Vec::new();
                };
                self.last_position = (point.x, point.y);
                vec![mouse_call(
                    "mouseMoved",
                    point.x,
                    point.y,
                    MouseButton::Left,
                    1,
                    0,
                    Modifiers::default(),
                )]
            }
            TouchPhase::End => {
                let ended = self.mouse_finger.is_some_and(|finger| {
                    points.is_empty() || points.iter().any(|point| point.id == finger)
                });
                if !ended {
                    return Vec::new();
                }
                self.mouse_finger = None;
                self.held = MouseButton::None;
                let (x, y) = point.map_or(self.last_position, |point| (point.x, point.y));
                let count = self.last_press.map_or(1, |press| press.count);
                vec![mouse_call(
                    "mouseReleased",
                    x,
                    y,
                    MouseButton::Left,
                    0,
                    count,
                    Modifiers::default(),
                )]
            }
            TouchPhase::Cancel => {
                if self.mouse_finger.take().is_none() {
                    return Vec::new();
                }
                self.held = MouseButton::None;
                self.last_press = None;
                // Released away from where it was pressed, so that no click follows.
                vec![mouse_call(
                    "mouseReleased",
                    -1.0,
                    -1.0,
                    MouseButton::Left,
                    0,
                    1,
                    Modifiers::default(),
                )]
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mouse(
        &mut self,
        action: MouseAction,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        clicks: Option<u32>,
        (dx, dy): (f64, f64),
        modifiers: Modifiers,
    ) -> Vec<Call> {
        self.last_position = (x, y);
        match action {
            MouseAction::Move => {
                if let Some(held) = button {
                    self.held = held;
                }
                vec![mouse_call(
                    "mouseMoved",
                    x,
                    y,
                    self.held,
                    self.held.bit(),
                    0,
                    modifiers,
                )]
            }
            MouseAction::Down => {
                let button = match button.unwrap_or_default() {
                    MouseButton::None => MouseButton::Left,
                    button => button,
                };
                self.held = button;
                vec![mouse_call(
                    "mousePressed",
                    x,
                    y,
                    button,
                    button.bit(),
                    clicks.unwrap_or(1),
                    modifiers,
                )]
            }
            MouseAction::Up => {
                let button = match button.unwrap_or_default() {
                    MouseButton::None => self.held,
                    button => button,
                };
                self.held = MouseButton::None;
                vec![mouse_call(
                    "mouseReleased",
                    x,
                    y,
                    button,
                    0,
                    clicks.unwrap_or(1),
                    modifiers,
                )]
            }
            MouseAction::Wheel => vec![wheel(x, y, dx, dy, modifiers)],
        }
    }

    fn click_count(&mut self, x: f64, y: f64, now: Instant) -> u32 {
        let count = match self.last_press {
            Some(press)
                if now.saturating_duration_since(press.at) <= MULTI_CLICK_INTERVAL
                    && (press.x - x).hypot(press.y - y) <= MULTI_CLICK_DISTANCE =>
            {
                press.count + 1
            }
            Some(_) | None => 1,
        };
        self.last_press = Some(Press {
            at: now,
            x,
            y,
            count,
        });
        count
    }
}

fn touch_call(kind: &str, points: &[TouchPoint]) -> Call {
    let points: Vec<Value> = points
        .iter()
        .map(|point| json!({ "x": point.x, "y": point.y, "id": point.id }))
        .collect();
    Call::new(
        "Input.dispatchTouchEvent",
        json!({ "type": kind, "touchPoints": points }),
    )
}

fn mouse_call(
    kind: &str,
    x: f64,
    y: f64,
    button: MouseButton,
    buttons: u32,
    click_count: u32,
    modifiers: Modifiers,
) -> Call {
    Call::new(
        "Input.dispatchMouseEvent",
        json!({
            "type": kind,
            "x": x,
            "y": y,
            "button": button.name(),
            "buttons": buttons,
            "clickCount": click_count,
            "modifiers": modifiers.0,
        }),
    )
}

fn wheel(x: f64, y: f64, dx: f64, dy: f64, modifiers: Modifiers) -> Call {
    Call::new(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel",
            "x": x,
            "y": y,
            "deltaX": dx,
            "deltaY": dy,
            "modifiers": modifiers.0,
        }),
    )
}

/// What a key is to the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    pub key: String,
    pub code: String,
    /// `KeyboardEvent.keyCode`, which the protocol calls `windowsVirtualKeyCode`.
    pub key_code: u32,
    /// The text the key types, without modifiers that suppress it.
    pub text: Option<String>,
    /// 1 for the left one of a pair of modifier keys.
    pub location: u32,
}

/// The browser's view of the key named `key` (and `code`, when the client knows it).
pub fn key_info(key: &str, code: Option<&str>) -> KeyInfo {
    let named = named_key(key);
    let code = code
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .or_else(|| named.map(|(_, code, _)| code.to_owned()))
        .or_else(|| code_of_character(key))
        .unwrap_or_default();
    let key_code = named
        .map(|(key_code, _, _)| key_code)
        .or_else(|| key_code_of_code(&code))
        .or_else(|| key_code_of_character(key))
        .unwrap_or(0);
    let text = match named {
        Some((_, _, text)) => text.map(str::to_owned),
        None if key.chars().count() == 1 => Some(key.to_owned()),
        None => None,
    };
    let location = match key {
        "Shift" | "Control" | "Alt" | "Meta" => 1,
        _ => 0,
    };
    KeyInfo {
        key: key.to_owned(),
        code,
        key_code,
        text,
        location,
    }
}

/// Key code, code and text of the keys with names.
fn named_key(key: &str) -> Option<(u32, &'static str, Option<&'static str>)> {
    let named = match key {
        "Backspace" => (8, "Backspace", None),
        "Tab" => (9, "Tab", None),
        "Enter" => (13, "Enter", Some("\r")),
        "Shift" => (16, "ShiftLeft", None),
        "Control" => (17, "ControlLeft", None),
        "Alt" => (18, "AltLeft", None),
        "Pause" => (19, "Pause", None),
        "CapsLock" => (20, "CapsLock", None),
        "Escape" => (27, "Escape", None),
        " " => (32, "Space", Some(" ")),
        "PageUp" => (33, "PageUp", None),
        "PageDown" => (34, "PageDown", None),
        "End" => (35, "End", None),
        "Home" => (36, "Home", None),
        "ArrowLeft" => (37, "ArrowLeft", None),
        "ArrowUp" => (38, "ArrowUp", None),
        "ArrowRight" => (39, "ArrowRight", None),
        "ArrowDown" => (40, "ArrowDown", None),
        "Insert" => (45, "Insert", None),
        "Delete" => (46, "Delete", None),
        "Meta" => (91, "MetaLeft", None),
        "ContextMenu" => (93, "ContextMenu", None),
        _ => return function_key(key).map(|(key_code, code)| (key_code, code, None)),
    };
    Some(named)
}

fn function_key(key: &str) -> Option<(u32, &'static str)> {
    const CODES: [&str; 12] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    ];
    let index = CODES.iter().position(|code| *code == key)?;
    Some((112 + index as u32, CODES[index]))
}

fn key_code_of_code(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key") {
        let letter = single_char(letter)?;
        return letter.is_ascii_uppercase().then_some(letter as u32);
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let digit = single_char(digit)?;
        return digit.is_ascii_digit().then_some(digit as u32);
    }
    if let Some(digit) = code.strip_prefix("Numpad") {
        if let Some(digit) = single_char(digit).filter(char::is_ascii_digit) {
            return Some(96 + (digit as u32 - '0' as u32));
        }
    }
    let key_code = match code {
        "Semicolon" => 186,
        "Equal" => 187,
        "Comma" => 188,
        "Minus" => 189,
        "Period" => 190,
        "Slash" => 191,
        "Backquote" => 192,
        "BracketLeft" => 219,
        "Backslash" => 220,
        "BracketRight" => 221,
        "Quote" => 222,
        "Space" => 32,
        _ => return None,
    };
    Some(key_code)
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

/// The US keyboard key that types `key`, for a client that does not send the code.
fn code_of_character(key: &str) -> Option<String> {
    let character = single_char(key)?;
    if character.is_ascii_alphabetic() {
        return Some(format!("Key{}", character.to_ascii_uppercase()));
    }
    if character.is_ascii_digit() {
        return Some(format!("Digit{character}"));
    }
    let code = match character {
        ';' | ':' => "Semicolon",
        '=' | '+' => "Equal",
        ',' | '<' => "Comma",
        '-' | '_' => "Minus",
        '.' | '>' => "Period",
        '/' | '?' => "Slash",
        '`' | '~' => "Backquote",
        '[' | '{' => "BracketLeft",
        '\\' | '|' => "Backslash",
        ']' | '}' => "BracketRight",
        '\'' | '"' => "Quote",
        '!' => "Digit1",
        '@' => "Digit2",
        '#' => "Digit3",
        '$' => "Digit4",
        '%' => "Digit5",
        '^' => "Digit6",
        '&' => "Digit7",
        '*' => "Digit8",
        '(' => "Digit9",
        ')' => "Digit0",
        _ => return None,
    };
    Some(code.to_owned())
}

fn key_code_of_character(key: &str) -> Option<u32> {
    key_code_of_code(&code_of_character(key)?)
}

/// The macOS editing command a shortcut performs. Chrome on the Mac takes these from the menu
/// and the text system, which keys sent through the protocol bypass.
pub fn mac_command(key: &str, modifiers: Modifiers) -> Option<&'static str> {
    let shift = modifiers.has(Modifiers::SHIFT);
    let meta = modifiers.has(Modifiers::META);
    let alt = modifiers.has(Modifiers::ALT);
    if modifiers.has(Modifiers::CTRL) || (meta && alt) {
        return None;
    }
    let letter = single_char(key).map(|c| c.to_ascii_lowercase());
    let command = match (meta, alt, shift, letter, key) {
        (true, false, false, Some('a'), _) => "selectAll",
        (true, false, false, Some('c'), _) => "copy",
        (true, false, false, Some('x'), _) => "cut",
        (true, false, false, Some('v'), _) => "paste",
        (true, false, false, Some('z'), _) => "undo",
        (true, false, true, Some('z'), _) => "redo",
        (true, false, false, _, "ArrowLeft") => "moveToLeftEndOfLine",
        (true, false, true, _, "ArrowLeft") => "moveToLeftEndOfLineAndModifySelection",
        (true, false, false, _, "ArrowRight") => "moveToRightEndOfLine",
        (true, false, true, _, "ArrowRight") => "moveToRightEndOfLineAndModifySelection",
        (true, false, false, _, "ArrowUp") => "moveToBeginningOfDocument",
        (true, false, true, _, "ArrowUp") => "moveToBeginningOfDocumentAndModifySelection",
        (true, false, false, _, "ArrowDown") => "moveToEndOfDocument",
        (true, false, true, _, "ArrowDown") => "moveToEndOfDocumentAndModifySelection",
        (true, false, _, _, "Backspace") => "deleteToBeginningOfLine",
        (false, true, false, _, "ArrowLeft") => "moveWordLeft",
        (false, true, true, _, "ArrowLeft") => "moveWordLeftAndModifySelection",
        (false, true, false, _, "ArrowRight") => "moveWordRight",
        (false, true, true, _, "ArrowRight") => "moveWordRightAndModifySelection",
        (false, true, _, _, "Backspace") => "deleteWordBackward",
        (false, true, _, _, "Delete") => "deleteWordForward",
        _ => return None,
    };
    Some(command)
}

fn key_calls(key: &str, code: Option<&str>, modifiers: Modifiers, action: KeyAction) -> Vec<Call> {
    let info = key_info(key, code);
    // Cmd and Ctrl shortcuts type nothing.
    let text = info
        .text
        .clone()
        .filter(|_| !modifiers.has(Modifiers::META) && !modifiers.has(Modifiers::CTRL));
    let command = mac_command(key, modifiers);
    let mut down = json!({
        "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": info.key,
        "code": info.code,
        "windowsVirtualKeyCode": info.key_code,
        "nativeVirtualKeyCode": info.key_code,
        "modifiers": modifiers.0,
        "location": info.location,
    });
    if let Some(text) = &text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    if let Some(command) = command {
        down["commands"] = json!([command]);
    }
    let up = json!({
        "type": "keyUp",
        "key": info.key,
        "code": info.code,
        "windowsVirtualKeyCode": info.key_code,
        "nativeVirtualKeyCode": info.key_code,
        "modifiers": modifiers.0,
        "location": info.location,
    });
    let down = Call::new("Input.dispatchKeyEvent", down);
    let up = Call::new("Input.dispatchKeyEvent", up);
    match action {
        KeyAction::Press => vec![down, up],
        KeyAction::Down => vec![down],
        KeyAction::Up => vec![up],
    }
}

#[cfg(test)]
#[path = "browser_input_tests.rs"]
mod tests;
