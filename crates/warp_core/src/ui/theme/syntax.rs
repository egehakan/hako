//! Syntax highlighting styles a theme can carry for code.

use std::collections::BTreeMap;
use std::ops::Not;

use serde::{Deserialize, Serialize};
use warpui_core::color::ColorU;

use crate::ui::color::hex_color::AlphaHexColor;

/// Style of the code that one highlight capture covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SerializedSyntaxStyle", into = "SerializedSyntaxStyle")]
pub struct SyntaxStyle {
    /// Left unset to keep the color the capture has without the theme's style.
    pub color: Option<ColorU>,
    pub underline: bool,
    /// Accepted so a theme can state it; the editor does not render italic or bold code.
    pub italic: bool,
    /// Accepted so a theme can state it; the editor does not render italic or bold code.
    pub bold: bool,
}

impl From<ColorU> for SyntaxStyle {
    fn from(color: ColorU) -> Self {
        Self {
            color: Some(color),
            ..Default::default()
        }
    }
}

/// A style is written as a bare color, or as a map when it also names font styles.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SerializedSyntaxStyle {
    Color(AlphaHexColor),
    Styled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<AlphaHexColor>,
        #[serde(default, skip_serializing_if = "Not::not")]
        underline: bool,
        #[serde(default, skip_serializing_if = "Not::not")]
        italic: bool,
        #[serde(default, skip_serializing_if = "Not::not")]
        bold: bool,
    },
}

impl From<SerializedSyntaxStyle> for SyntaxStyle {
    fn from(style: SerializedSyntaxStyle) -> Self {
        match style {
            SerializedSyntaxStyle::Color(color) => ColorU::from(color).into(),
            SerializedSyntaxStyle::Styled {
                color,
                underline,
                italic,
                bold,
            } => Self {
                color: color.map(ColorU::from),
                underline,
                italic,
                bold,
            },
        }
    }
}

impl From<SyntaxStyle> for SerializedSyntaxStyle {
    fn from(style: SyntaxStyle) -> Self {
        match style {
            SyntaxStyle {
                color: Some(color),
                underline: false,
                italic: false,
                bold: false,
            } => Self::Color(AlphaHexColor(color)),
            SyntaxStyle {
                color,
                underline,
                italic,
                bold,
            } => Self::Styled {
                color: color.map(AlphaHexColor),
                underline,
                italic,
                bold,
            },
        }
    }
}

/// Styles keyed by the name of the highlight capture they apply to, such as `keyword` or
/// `function.method`. A style also applies to every capture nested under its name unless a
/// longer name styles that capture.
pub type SyntaxStyles = BTreeMap<String, SyntaxStyle>;

/// The syntax highlighting section of a theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxTheme {
    /// Styles for every language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    captures: SyntaxStyles,
    /// Styles for one language each, keyed by language name. They take precedence over
    /// equally specific entries of `captures`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    languages: BTreeMap<String, SyntaxStyles>,
}

impl SyntaxTheme {
    pub fn new(captures: SyntaxStyles, languages: BTreeMap<String, SyntaxStyles>) -> Self {
        Self {
            captures,
            languages,
        }
    }

    pub fn captures(&self) -> &SyntaxStyles {
        &self.captures
    }

    pub fn languages(&self) -> &BTreeMap<String, SyntaxStyles> {
        &self.languages
    }
}

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
