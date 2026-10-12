use std::collections::BTreeMap;

use warpui_core::color::ColorU;

use super::{SyntaxStyle, SyntaxTheme};

const SYNTAX_YAML: &str = r##"---
captures:
  keyword: "#102030"
  text.uri:
    color: "#405060"
    underline: true
languages:
  python:
    variable: "#70809080"
"##;

fn style(yaml: &str) -> SyntaxStyle {
    serde_yaml::from_str(yaml).expect("style should deserialize")
}

#[test]
fn styles_deserialize_from_a_color_or_a_map() {
    let color = ColorU::new(0x10, 0x20, 0x30, 0xff);
    assert_eq!(style("\"#102030\""), SyntaxStyle::from(color));
    assert_eq!(
        style("{ color: \"#102030\", underline: true, italic: true }"),
        SyntaxStyle {
            color: Some(color),
            underline: true,
            italic: true,
            bold: false,
        },
    );
    assert_eq!(
        style("{ bold: true }"),
        SyntaxStyle {
            bold: true,
            ..Default::default()
        },
    );
}

#[test]
fn style_colors_may_carry_alpha() {
    assert_eq!(
        style("\"#10203040\""),
        SyntaxStyle::from(ColorU::new(0x10, 0x20, 0x30, 0x40)),
    );
}

#[test]
fn invalid_style_colors_are_rejected() {
    for yaml in ["\"102030\"", "\"#1020\"", "{ color: \"#xyzxyz\" }"] {
        assert!(
            serde_yaml::from_str::<SyntaxStyle>(yaml).is_err(),
            "{yaml} should not deserialize",
        );
    }
}

#[test]
fn syntax_theme_round_trips() {
    let theme: SyntaxTheme = serde_yaml::from_str(SYNTAX_YAML).expect("should deserialize");

    let expected = SyntaxTheme::new(
        BTreeMap::from([
            (
                "keyword".to_string(),
                ColorU::new(0x10, 0x20, 0x30, 0xff).into(),
            ),
            (
                "text.uri".to_string(),
                SyntaxStyle {
                    color: Some(ColorU::new(0x40, 0x50, 0x60, 0xff)),
                    underline: true,
                    ..Default::default()
                },
            ),
        ]),
        BTreeMap::from([(
            "python".to_string(),
            BTreeMap::from([(
                "variable".to_string(),
                ColorU::new(0x70, 0x80, 0x90, 0x80).into(),
            )]),
        )]),
    );
    assert_eq!(theme, expected);

    assert_eq!(
        serde_yaml::to_string(&theme).expect("should serialize"),
        SYNTAX_YAML,
    );
}

#[test]
fn capture_names_are_not_validated() {
    let theme: SyntaxTheme =
        serde_yaml::from_str("captures:\n  no.such.capture: \"#102030\"\n  \"\": \"#102030\"\n")
            .expect("unknown capture names should deserialize");
    assert_eq!(theme.captures().len(), 2);
    assert!(theme.languages().is_empty());
}
