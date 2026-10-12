use warpui_core::color::ColorU;

use crate::ui::theme::{AnsiColorIdentifier, Fill, WarpTheme, mock_terminal_colors};

const BASE_THEME_YAML: &str = r##"---
background: "#101010"
accent: "#202020"
foreground: "#f0f0f0"
details: darker
terminal_colors:
  normal:
    black: "#616161"
    red: "#ff8272"
    green: "#b4fa72"
    yellow: "#fefdc2"
    blue: "#a5d5fe"
    magenta: "#ff8ffd"
    cyan: "#d0d1fe"
    white: "#f1f1f1"
  bright:
    black: "#8e8e8e"
    red: "#ffc4bd"
    green: "#d6fcb9"
    yellow: "#fefdd5"
    blue: "#c1e3fe"
    magenta: "#ffb1fe"
    cyan: "#e5e6fe"
    white: "#feffff"
"##;

const EDITOR_YAML: &str = r##"editor:
  foreground: "#010101"
  cursor: "#020202"
  selection: "#03030380"
  current_line: "#040404"
  line_number: "#050505"
  active_line_number: "#060606"
  find_match: "#07070740"
  selected_find_match: "#080808"
  diff_added: "#090909"
  diff_removed: "#0a0a0a"
  diff_modified: "#0b0b0b"
  diff_added_line: "#0c0c0c33"
  diff_added_text: "#0d0d0d33"
  diff_removed_line: "#0e0e0e33"
  diff_removed_text: "#0f0f0f33"
"##;

fn theme(extra_yaml: &str) -> WarpTheme {
    serde_yaml::from_str(&format!("{BASE_THEME_YAML}{extra_yaml}"))
        .expect("theme should deserialize")
}

fn gray(value: u8, alpha: u8) -> ColorU {
    ColorU::new(value, value, value, alpha)
}

#[test]
fn unset_editor_colors_keep_their_derived_values() {
    for theme in [theme(""), theme("editor: {}\n")] {
        let background = theme.background();
        let ansi = mock_terminal_colors().normal;

        assert_eq!(
            theme.editor_foreground(),
            theme.main_text_color(background).into_solid(),
        );
        assert_eq!(theme.editor_selection(), theme.text_selection_color());
        assert_eq!(theme.editor_current_line(), theme.surface_2());
        assert_eq!(
            theme.editor_line_number(),
            theme.sub_text_color(background).into_solid(),
        );
        assert_eq!(
            theme.editor_active_line_number(),
            theme.main_text_color(background).into_solid(),
        );
        assert_eq!(
            theme.editor_diff_added(),
            AnsiColorIdentifier::Green.to_ansi_color(&ansi).into(),
        );
        assert_eq!(
            theme.editor_diff_removed(),
            AnsiColorIdentifier::Red.to_ansi_color(&ansi).into(),
        );
        assert_eq!(
            theme.editor_diff_modified(),
            AnsiColorIdentifier::Yellow.to_ansi_color(&ansi).into(),
        );

        assert_eq!(theme.editor_cursor(), None);
        assert_eq!(theme.editor_find_match(), None);
        assert_eq!(theme.editor_selected_find_match(), None);
        assert_eq!(theme.editor_diff_added_line(), None);
        assert_eq!(theme.editor_diff_added_text(), None);
        assert_eq!(theme.editor_diff_removed_line(), None);
        assert_eq!(theme.editor_diff_removed_text(), None);
    }
}

#[test]
fn set_editor_colors_replace_the_derived_values() {
    let theme = theme(EDITOR_YAML);

    assert_eq!(theme.editor_foreground(), gray(0x01, 0xff));
    assert_eq!(theme.editor_cursor(), Some(Fill::Solid(gray(0x02, 0xff))));
    assert_eq!(theme.editor_selection(), Fill::Solid(gray(0x03, 0x80)));
    assert_eq!(theme.editor_current_line(), Fill::Solid(gray(0x04, 0xff)));
    assert_eq!(theme.editor_line_number(), gray(0x05, 0xff));
    assert_eq!(theme.editor_active_line_number(), gray(0x06, 0xff));
    assert_eq!(
        theme.editor_find_match(),
        Some(Fill::Solid(gray(0x07, 0x40)))
    );
    assert_eq!(
        theme.editor_selected_find_match(),
        Some(Fill::Solid(gray(0x08, 0xff))),
    );
    assert_eq!(theme.editor_diff_added(), gray(0x09, 0xff));
    assert_eq!(theme.editor_diff_removed(), gray(0x0a, 0xff));
    assert_eq!(theme.editor_diff_modified(), gray(0x0b, 0xff));
    assert_eq!(theme.editor_diff_added_line(), Some(gray(0x0c, 0x33)));
    assert_eq!(theme.editor_diff_added_text(), Some(gray(0x0d, 0x33)));
    assert_eq!(theme.editor_diff_removed_line(), Some(gray(0x0e, 0x33)));
    assert_eq!(theme.editor_diff_removed_text(), Some(gray(0x0f, 0x33)));
}

#[test]
fn a_partial_editor_section_overrides_only_what_it_names() {
    let theme = theme("editor:\n  line_number: \"#050505\"\n");

    assert_eq!(theme.editor_line_number(), gray(0x05, 0xff));
    assert_eq!(theme.editor_current_line(), theme.surface_2());
    assert_eq!(theme.editor_find_match(), None);
}

#[test]
fn editor_colors_round_trip() {
    let theme = theme(EDITOR_YAML);
    let serialized = serde_yaml::to_string(&theme).expect("theme should serialize");

    assert!(
        serialized.ends_with(EDITOR_YAML),
        "the editor section should serialize as it was written, got:\n{serialized}",
    );
    assert_eq!(
        serde_yaml::from_str::<WarpTheme>(&serialized).expect("theme should deserialize"),
        theme,
    );
}
