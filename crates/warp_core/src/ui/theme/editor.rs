//! Colors a theme can set for the code editor, in place of the ones derived from its base colors.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use warpui_core::color::ColorU;

use super::{AnsiColorIdentifier, Fill, WarpTheme};
use crate::ui::color::hex_color::AlphaHexColor;

/// The code editor section of a theme. Each color left unset keeps its derived value.
#[skip_serializing_none]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorColors {
    /// Code that no syntax style colors.
    foreground: Option<AlphaHexColor>,
    cursor: Option<AlphaHexColor>,
    selection: Option<AlphaHexColor>,
    /// Background of the line the cursor is on.
    current_line: Option<AlphaHexColor>,
    line_number: Option<AlphaHexColor>,
    /// Number of the line the cursor is on.
    active_line_number: Option<AlphaHexColor>,
    /// Background of each search match other than the selected one.
    find_match: Option<AlphaHexColor>,
    selected_find_match: Option<AlphaHexColor>,
    /// Gutter mark of added lines.
    diff_added: Option<AlphaHexColor>,
    /// Gutter mark of removed lines.
    diff_removed: Option<AlphaHexColor>,
    /// Gutter mark of changed lines.
    diff_modified: Option<AlphaHexColor>,
    /// Background of added lines.
    diff_added_line: Option<AlphaHexColor>,
    /// Background of the added part of a changed line.
    diff_added_text: Option<AlphaHexColor>,
    /// Background of removed lines.
    diff_removed_line: Option<AlphaHexColor>,
    /// Background of the removed part of a changed line.
    diff_removed_text: Option<AlphaHexColor>,
}

/// Code editor colors. The ones returning an `Option` have no default that derives from the
/// theme's base colors, so the editor supplies its own when the theme sets none.
impl WarpTheme {
    fn editor_colors(&self) -> EditorColors {
        self.editor.unwrap_or_default()
    }

    fn ansi_color(&self, identifier: AnsiColorIdentifier) -> ColorU {
        identifier
            .to_ansi_color(&self.terminal_colors().normal)
            .into()
    }

    pub fn editor_foreground(&self) -> ColorU {
        self.editor_colors()
            .foreground
            .map(ColorU::from)
            .unwrap_or_else(|| self.main_text_color(self.background()).into_solid())
    }

    pub fn editor_cursor(&self) -> Option<Fill> {
        self.editor_colors().cursor.map(Fill::from)
    }

    pub fn editor_selection(&self) -> Fill {
        self.editor_colors()
            .selection
            .map_or_else(|| self.text_selection_color(), Fill::from)
    }

    pub fn editor_current_line(&self) -> Fill {
        self.editor_colors()
            .current_line
            .map_or_else(|| self.surface_2(), Fill::from)
    }

    pub fn editor_line_number(&self) -> ColorU {
        self.editor_colors()
            .line_number
            .map(ColorU::from)
            .unwrap_or_else(|| self.sub_text_color(self.background()).into_solid())
    }

    pub fn editor_active_line_number(&self) -> ColorU {
        self.editor_colors()
            .active_line_number
            .map(ColorU::from)
            .unwrap_or_else(|| self.main_text_color(self.background()).into_solid())
    }

    pub fn editor_find_match(&self) -> Option<Fill> {
        self.editor_colors().find_match.map(Fill::from)
    }

    pub fn editor_selected_find_match(&self) -> Option<Fill> {
        self.editor_colors().selected_find_match.map(Fill::from)
    }

    pub fn editor_diff_added(&self) -> ColorU {
        self.editor_colors()
            .diff_added
            .map(ColorU::from)
            .unwrap_or_else(|| self.ansi_color(AnsiColorIdentifier::Green))
    }

    pub fn editor_diff_removed(&self) -> ColorU {
        self.editor_colors()
            .diff_removed
            .map(ColorU::from)
            .unwrap_or_else(|| self.ansi_color(AnsiColorIdentifier::Red))
    }

    pub fn editor_diff_modified(&self) -> ColorU {
        self.editor_colors()
            .diff_modified
            .map(ColorU::from)
            .unwrap_or_else(|| self.ansi_color(AnsiColorIdentifier::Yellow))
    }

    pub fn editor_diff_added_line(&self) -> Option<ColorU> {
        self.editor_colors().diff_added_line.map(ColorU::from)
    }

    pub fn editor_diff_added_text(&self) -> Option<ColorU> {
        self.editor_colors().diff_added_text.map(ColorU::from)
    }

    pub fn editor_diff_removed_line(&self) -> Option<ColorU> {
        self.editor_colors().diff_removed_line.map(ColorU::from)
    }

    pub fn editor_diff_removed_text(&self) -> Option<ColorU> {
        self.editor_colors().diff_removed_text.map(ColorU::from)
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
