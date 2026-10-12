use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use warpui_core::color::ColorU;

use super::OPAQUE;

const SHORT_COLOR_LEN: usize = 3;
const FULL_COLOR_LEN: usize = 6;
const ALPHA_COLOR_LEN: usize = 8;

#[derive(PartialEq, Eq, Debug, Clone, thiserror::Error)]
pub enum HexColorError {
    #[error("Expected hex color string starting with #.")]
    HashPrefix,
    #[error("Expected hex color string starting with # followed by 3 or 6 characters.")]
    InvalidLength,
    #[error("Invalid hex color string")]
    InvalidValue,
}

pub fn coloru_from_hex_string(s: &str) -> Result<ColorU, HexColorError> {
    if !s.starts_with('#') {
        return Err(HexColorError::HashPrefix);
    }
    let mut s: Cow<str> = s[1..].into();

    if s.len() != SHORT_COLOR_LEN && s.len() != FULL_COLOR_LEN {
        return Err(HexColorError::InvalidLength);
    }

    // for a shorter color representation we want to "normalize" it to the standard 6-character
    // one, so #123 becomes #112233.
    if s.len() == SHORT_COLOR_LEN {
        s = s
            .chars()
            .flat_map(|c| std::iter::repeat_n(c, 2))
            .collect::<String>()
            .into();
    }

    let parsed = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>();

    match parsed {
        Ok(bytes) if bytes.len() == 3 => Ok(ColorU {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: OPAQUE,
        }),
        _ => Err(HexColorError::InvalidValue),
    }
}

pub fn coloru_to_hex_string(coloru: &ColorU) -> String {
    format!("#{:02x}{:02x}{:02x}", coloru.r, coloru.g, coloru.b)
}

/// Parses `#rgb` and `#rrggbb` like [`coloru_from_hex_string`], and also `#rrggbbaa`.
pub fn coloru_from_hex_string_with_alpha(s: &str) -> Result<ColorU, HexColorError> {
    let Some(digits) = s.strip_prefix('#') else {
        return Err(HexColorError::HashPrefix);
    };
    if digits.len() != ALPHA_COLOR_LEN {
        return coloru_from_hex_string(s);
    }
    if !digits.is_ascii() {
        return Err(HexColorError::InvalidValue);
    }

    let (rgb, alpha) = digits.split_at(FULL_COLOR_LEN);
    let opaque = coloru_from_hex_string(&format!("#{rgb}"))?;
    let alpha = u8::from_str_radix(alpha, 16).map_err(|_| HexColorError::InvalidValue)?;
    Ok(ColorU::new(opaque.r, opaque.g, opaque.b, alpha))
}

/// Formats a color as `#rrggbb`, or as `#rrggbbaa` when it is not opaque.
pub fn coloru_to_hex_string_with_alpha(coloru: &ColorU) -> String {
    if coloru.a == OPAQUE {
        coloru_to_hex_string(coloru)
    } else {
        format!("{}{:02x}", coloru_to_hex_string(coloru), coloru.a)
    }
}

/// A color that is written as a hex string and may carry an alpha channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaHexColor(pub ColorU);

impl From<AlphaHexColor> for ColorU {
    fn from(color: AlphaHexColor) -> Self {
        color.0
    }
}

impl Serialize for AlphaHexColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        coloru_to_hex_string_with_alpha(&self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AlphaHexColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        coloru_from_hex_string_with_alpha(&s)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

pub fn deserialize<'de, D, C>(deserializer: D) -> Result<C, D::Error>
where
    C: From<ColorU>,
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    coloru_from_hex_string(&s)
        .map(Into::into)
        .map_err(de::Error::custom)
}

pub fn serialize<S, C>(color: &C, serializer: S) -> Result<S::Ok, S::Error>
where
    C: Into<ColorU> + Clone,
    S: Serializer,
{
    let coloru: ColorU = color.to_owned().into();
    coloru_to_hex_string(&coloru).serialize(serializer)
}
