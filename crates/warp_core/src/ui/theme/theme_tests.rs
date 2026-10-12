use super::*;

#[test]
fn serialize_test() {
    let theme = WarpTheme::new(
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        ColorU::from_u32(0x20A5BAFF),
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        None,
        Some(Details::Darker),
        mock_terminal_colors(),
        None,
        Some("test_theme".to_string()),
    );
    assert_eq!(
        r##"---
background: "#20a5ba"
accent: "#20a5ba"
foreground: "#20a5ba"
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
name: test_theme
"##,
        serde_yaml::to_string(&theme).expect("Couldn't serialize")
    );
}

#[test]
fn deserialize_with_name_test() {
    let theme = serde_yaml::from_str::<WarpTheme>(
        r##"---
background: "#20a5ba"
accent: "#20a5ba"
foreground: "#20a5ba"
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
name: test_theme
"##,
    )
    .expect("Couldn't deserialize");

    let expected_theme = WarpTheme::new(
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        ColorU::from_u32(0x20A5BAFF),
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        None,
        Some(Details::Darker),
        mock_terminal_colors(),
        None,
        Some("test_theme".to_string()),
    );

    assert_eq!(expected_theme, theme);
}

#[test]
fn deserialize_without_name_test() {
    let theme = serde_yaml::from_str::<WarpTheme>(
        r##"---
background: "#20a5ba"
accent: "#20a5ba"
foreground: "#20a5ba"
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
"##,
    )
    .expect("Couldn't deserialize");

    let expected_theme = WarpTheme::new(
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        ColorU::from_u32(0x20A5BAFF),
        Fill::Solid(ColorU::from_u32(0x20A5BAFF)),
        None,
        Some(Details::Darker),
        mock_terminal_colors(),
        None,
        None,
    );

    assert_eq!(expected_theme, theme);
}

#[test]
fn blend_gradient_test() {
    let (c1, c2, c3, c4) = (
        ColorU::from_u32(0x002b36ff),
        ColorU::from_u32(0xcb4b16ff),
        ColorU::from_u32(0xffffff19),
        ColorU::from_u32(0xffffff19),
    );
    let g1 = VerticalGradient::new(c1, c2);
    let g2 = VerticalGradient::new(c3, c4);

    assert_eq!(
        g1.blend(&g2),
        VerticalGradient::new(c1.blend(&c3), c2.blend(&c4))
    );
}

#[test]
fn blend_coloru_test() {
    let c1 = ColorU::from_u32(0x002b36ff);
    let c2 = ColorU::from_u32(0xF8F8F2FF);
    assert_eq!(
        c1.blend(&coloru_with_opacity(c2, 10)),
        ColorU::from_u32(0x183f48ff)
    );
    assert_eq!(
        ColorU::from_u32(0x000000ff).blend(&coloru_with_opacity(c2, 10)),
        ColorU::from_u32(0x181818ff)
    );
}

/// TODO(CORE-3626): write an equivalent test with Windows paths.
#[cfg(not(windows))]
#[test]
fn test_deserialize_image() {
    // Paths that start with `~` should expand to include the home dir.
    let a = "
    path: ~/warp.jpg
    opacity: 60
    ";
    let image: Image = serde_yaml::from_str(a).unwrap();
    assert_eq!(image.opacity, 60);
    assert_eq!(
        image.source,
        AssetSource::LocalFile {
            path: home_dir()
                .unwrap()
                .join("warp.jpg")
                .to_str()
                .unwrap_or_default()
                .to_owned(),
            content_version: None,
        }
    );

    // Absolute paths should be unchanged.
    let b = "
    path: /warp.jpg
    opacity: 60
    ";
    let image: Image = serde_yaml::from_str(b).unwrap();
    assert_eq!(image.opacity, 60);
    assert_eq!(
        image.source,
        AssetSource::LocalFile {
            path: "/warp.jpg".to_owned(),
            content_version: None,
        }
    );

    // Relative paths should expand to include the theme dir.
    let c = "
    path: warp.jpg
    opacity: 60
    ";
    let image: Image = serde_yaml::from_str(c).unwrap();
    assert_eq!(image.opacity, 60);
    assert_eq!(
        image.source,
        AssetSource::LocalFile {
            path: themes_dir()
                .join("warp.jpg")
                .to_str()
                .unwrap_or_default()
                .to_owned(),
            content_version: None,
        }
    );

    // No opacity should become the default
    let d = "
    path: warp.jpg
    ";
    let image: Image = serde_yaml::from_str(d).unwrap();
    assert_eq!(image.opacity, default_image_opacity());
}

#[test]
fn ansi_color_deserializing_test() {
    let raw = r##"
        black: "#000000"
        red: "#ff0000"
        green: "#00ff00"
        yellow: "#00ffff"
        blue: "#0000ff"
        magenta: "#ff0000"
        cyan: "#0000ff"
        white: "#ffffff"
        "##;
    let ansi_colors: AnsiColors = serde_yaml::from_str(raw).expect("Couldn't deserialize");
    assert_eq!(ansi_colors.black, AnsiColor::from_u32(0x000000ff));
    assert_eq!(ansi_colors.red, AnsiColor::from_u32(0xff0000ff));
    assert_eq!(ansi_colors.green, AnsiColor::from_u32(0x00ff00ff));
    assert_eq!(ansi_colors.yellow, AnsiColor::from_u32(0x00ffffff));
    assert_eq!(ansi_colors.blue, AnsiColor::from_u32(0x0000ffff));
    assert_eq!(ansi_colors.magenta, AnsiColor::from_u32(0xff0000ff));
    assert_eq!(ansi_colors.cyan, AnsiColor::from_u32(0x0000ffff));
    assert_eq!(ansi_colors.white, AnsiColor::from_u32(0xffffffff));
}

#[test]
fn ansi_color_serializing_test() {
    let ansi_colors = AnsiColors::new(
        AnsiColor::from_u32(0x000000ff),
        AnsiColor::from_u32(0xff0000ff),
        AnsiColor::from_u32(0x00ff00ff),
        AnsiColor::from_u32(0x00ffffff),
        AnsiColor::from_u32(0x0000ffff),
        AnsiColor::from_u32(0xff0000ff),
        AnsiColor::from_u32(0x0000ffff),
        AnsiColor::from_u32(0xffffffff),
    );
    let serialized = serde_yaml::to_string(&ansi_colors).expect("Couldn't serialize");
    let raw = r##"---
black: "#000000"
red: "#ff0000"
green: "#00ff00"
yellow: "#00ffff"
blue: "#0000ff"
magenta: "#ff0000"
cyan: "#0000ff"
white: "#ffffff"
"##;
    assert_eq!(serialized, raw);

    let ansi_colors2: AnsiColors = serde_yaml::from_str(&serialized).expect("Couldn't deserialize");
    assert_eq!(ansi_colors2, ansi_colors);
}

#[test]
fn from_hex_negative_test() {
    assert_eq!(
        hex_color::coloru_from_hex_string("#0").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#00").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#00000").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#0000000").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("0000").unwrap_err(),
        hex_color::HexColorError::HashPrefix
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#ZXD").unwrap_err(),
        hex_color::HexColorError::InvalidValue
    );
}

#[test]
fn from_hex_positive_test() {
    assert_eq!(
        hex_color::coloru_from_hex_string("#000").unwrap(),
        ColorU::from_u32(0x000000ff)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#000000").unwrap(),
        ColorU::from_u32(0x000000ff)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#123").unwrap(),
        ColorU::from_u32(0x112233ff)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#112233").unwrap(),
        ColorU::from_u32(0x112233ff)
    );
}

#[test]
fn from_hex_with_alpha_test() {
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("#11223344").unwrap(),
        ColorU::from_u32(0x11223344)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("#112233").unwrap(),
        ColorU::from_u32(0x112233ff)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("#123").unwrap(),
        ColorU::from_u32(0x112233ff)
    );
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("#1122334").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("#112233zz").unwrap_err(),
        hex_color::HexColorError::InvalidValue
    );
    assert_eq!(
        hex_color::coloru_from_hex_string_with_alpha("11223344").unwrap_err(),
        hex_color::HexColorError::HashPrefix
    );
    assert_eq!(
        hex_color::coloru_from_hex_string("#11223344").unwrap_err(),
        hex_color::HexColorError::InvalidLength
    );
}

#[test]
fn to_hex_with_alpha_test() {
    assert_eq!(
        hex_color::coloru_to_hex_string_with_alpha(&ColorU::from_u32(0x112233ff)),
        "#112233"
    );
    assert_eq!(
        hex_color::coloru_to_hex_string_with_alpha(&ColorU::from_u32(0x1122330a)),
        "#1122330a"
    );
}

/// The base fields of a theme, to which a test appends the optional sections it exercises.
const BASE_THEME_YAML: &str = r##"---
background: "#20a5ba"
accent: "#20a5ba"
foreground: "#20a5ba"
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
name: test_theme
"##;

#[test]
fn theme_without_optional_sections_has_no_syntax_styles() {
    let theme = serde_yaml::from_str::<WarpTheme>(BASE_THEME_YAML).expect("Couldn't deserialize");

    assert_eq!(theme.syntax(), None);
    assert_eq!(
        serde_yaml::to_string(&theme).expect("Couldn't serialize"),
        BASE_THEME_YAML
    );
}

#[test]
fn theme_with_optional_sections_round_trips() {
    let yaml = format!(
        r##"{BASE_THEME_YAML}syntax:
  captures:
    function.method: "#010203"
    keyword:
      color: "#040506"
      underline: true
  languages:
    rust:
      keyword: "#07080910"
editor:
  selection: "#0a0b0c0d"
"##
    );
    let theme = serde_yaml::from_str::<WarpTheme>(&yaml).expect("Couldn't deserialize");

    let syntax = theme.syntax().expect("theme should carry syntax styles");
    assert_eq!(
        syntax.captures()["function.method"].color,
        Some(ColorU::from_u32(0x010203ff))
    );
    assert!(syntax.captures()["keyword"].underline);
    assert_eq!(
        syntax.languages()["rust"]["keyword"].color,
        Some(ColorU::from_u32(0x07080910))
    );
    assert_eq!(
        theme.editor_selection(),
        Fill::Solid(ColorU::from_u32(0x0a0b0c0d))
    );

    assert_eq!(
        serde_yaml::to_string(&theme).expect("Couldn't serialize"),
        yaml
    );
}

#[test]
fn unknown_theme_fields_are_ignored() {
    let yaml = format!("{BASE_THEME_YAML}not_a_field: 1\nsyntax:\n  not_a_field: 2\n");
    let theme = serde_yaml::from_str::<WarpTheme>(&yaml).expect("Couldn't deserialize");

    let syntax = theme.syntax().expect("theme should carry syntax styles");
    assert!(syntax.captures().is_empty());
    assert!(syntax.languages().is_empty());
}

#[test]
fn infer_from_foreground_color_test() {
    assert_eq!(
        ColorScheme::infer_from_foreground_color(ColorU::white()),
        ColorScheme::LightOnDark
    );
    assert_eq!(
        ColorScheme::infer_from_foreground_color(ColorU::black()),
        ColorScheme::DarkOnLight
    );
}
