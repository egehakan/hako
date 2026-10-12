use std::sync::Arc;

use languages::language_by_name;
use string_offset::CharOffset;
use warp_editor::content::buffer::Buffer;
use warp_editor::content::selection_model::BufferSelectionModel;
use warp_editor::content::text::IndentBehavior;
use warpui_core::App;
use warpui_core::color::ColorU;

use super::{
    CaptureStyle, CaptureStyles, ColorMap, HighlightPalette, HighlightQuery, Highlights,
    convert_capture_name_to_color,
};
use crate::SyntaxTreeState;

const KEYWORD: ColorU = ColorU {
    r: 1,
    g: 0,
    b: 0,
    a: 255,
};
const FUNCTION: ColorU = ColorU {
    r: 2,
    g: 0,
    b: 0,
    a: 255,
};
const STRING: ColorU = ColorU {
    r: 3,
    g: 0,
    b: 0,
    a: 255,
};
const TYPE: ColorU = ColorU {
    r: 4,
    g: 0,
    b: 0,
    a: 255,
};
const NUMBER: ColorU = ColorU {
    r: 5,
    g: 0,
    b: 0,
    a: 255,
};
const COMMENT: ColorU = ColorU {
    r: 6,
    g: 0,
    b: 0,
    a: 255,
};
const PROPERTY: ColorU = ColorU {
    r: 7,
    g: 0,
    b: 0,
    a: 255,
};
const TAG: ColorU = ColorU {
    r: 8,
    g: 0,
    b: 0,
    a: 255,
};

fn color_map() -> ColorMap {
    ColorMap {
        keyword_color: KEYWORD,
        function_color: FUNCTION,
        string_color: STRING,
        type_color: TYPE,
        number_color: NUMBER,
        comment_color: COMMENT,
        property_color: PROPERTY,
        tag_color: TAG,
    }
}

#[test]
fn capture_names_resolve_by_their_first_segment() {
    let cases = [
        ("keyword", KEYWORD),
        ("keyword.control.repeat", KEYWORD),
        ("function.call", FUNCTION),
        ("string.special", STRING),
        ("type.builtin", TYPE),
        ("number", NUMBER),
        ("comment", COMMENT),
        ("property", PROPERTY),
        ("tag", TAG),
        ("punctuation.bracket", COMMENT),
    ];
    for (name, expected) in cases {
        assert_eq!(
            convert_capture_name_to_color(name, &color_map()),
            Some(expected),
            "{name}",
        );
    }
}

#[test]
fn older_and_markup_capture_names_are_colored() {
    let cases = [
        ("conditional", KEYWORD),
        ("repeat", KEYWORD),
        ("include", KEYWORD),
        ("exception", KEYWORD),
        ("storageclass", KEYWORD),
        ("boolean", NUMBER),
        ("float", NUMBER),
        ("constant.numeric", NUMBER),
        ("method.call", FUNCTION),
        ("field", PROPERTY),
        ("attribute", PROPERTY),
        ("markup.heading.2", KEYWORD),
        ("markup.raw.block", STRING),
        ("text.title", KEYWORD),
        ("text.literal", STRING),
        ("text.uri", FUNCTION),
        ("text.reference", PROPERTY),
    ];
    for (name, expected) in cases {
        assert_eq!(
            convert_capture_name_to_color(name, &color_map()),
            Some(expected),
            "{name}",
        );
    }
}

#[test]
fn unclassified_capture_names_are_left_uncolored() {
    for name in [
        "variable",
        "operator",
        "constant",
        "constant.builtin",
        "markup.bold",
        "none",
    ] {
        assert_eq!(
            convert_capture_name_to_color(name, &color_map()),
            None,
            "{name}"
        );
    }
}

const THEME_STRING: ColorU = ColorU {
    r: 0,
    g: 1,
    b: 0,
    a: 255,
};
const THEME_EMBEDDED: ColorU = ColorU {
    r: 0,
    g: 2,
    b: 0,
    a: 255,
};
const THEME_VARIABLE: ColorU = ColorU {
    r: 0,
    g: 3,
    b: 0,
    a: 255,
};
const THEME_FUNCTION: ColorU = ColorU {
    r: 0,
    g: 4,
    b: 0,
    a: 255,
};
const THEME_METHOD: ColorU = ColorU {
    r: 0,
    g: 5,
    b: 0,
    a: 255,
};
const THEME_PYTHON_FUNCTION: ColorU = ColorU {
    r: 0,
    g: 6,
    b: 0,
    a: 255,
};

fn colored(color: ColorU) -> CaptureStyle {
    CaptureStyle {
        color: Some(color),
        underline: false,
    }
}

fn capture_styles(
    global: &[(&str, CaptureStyle)],
    languages: &[(&str, &[(&str, CaptureStyle)])],
) -> CaptureStyles {
    let owned = |styles: &[(&str, CaptureStyle)]| -> Vec<_> {
        styles
            .iter()
            .map(|(capture, style)| (capture.to_string(), *style))
            .collect()
    };
    CaptureStyles::new(
        owned(global),
        languages
            .iter()
            .map(|(language, styles)| (language.to_string(), owned(styles))),
    )
}

fn palette(styles: CaptureStyles) -> HighlightPalette {
    HighlightPalette {
        fallback: color_map(),
        styles: Some(Arc::new(styles)),
    }
}

/// Highlights `source` and returns each highlighted range as its text length and color, and the
/// lengths of the underlined ranges.
fn highlight(
    language: &str,
    source: &'static str,
    palette: HighlightPalette,
) -> (Vec<(usize, ColorU)>, Vec<usize>) {
    let language = language_by_name(language).expect("language should be supported");
    let Highlights { colors, underlines } = App::test((), |mut app| async move {
        let buffer_handle = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer_handle.clone()));
        buffer_handle.update(&mut app, |buffer, ctx| {
            *buffer = Buffer::from_plain_text(
                source,
                None,
                Box::new(|_, _| IndentBehavior::Ignore),
                selection,
                ctx,
            );
        });

        let snapshot = buffer_handle.read(&app, |buffer, _| buffer.buffer_snapshot());
        let tree = SyntaxTreeState::parse_text(snapshot, None, &language)
            .await
            .expect("test buffer is small and should parse");
        let query = HighlightQuery::new(&language.highlight_query, &palette, &language.name);

        buffer_handle.read(&app, |buffer, _| {
            query.get_highlighted_chunks(
                CharOffset::from(1)..buffer.max_charoffset(),
                &language.highlight_query,
                buffer,
                &tree,
            )
        })
    });

    (
        colors
            .iter()
            .map(|(range, color)| ((range.end - range.start).as_usize(), *color))
            .collect(),
        underlines
            .iter()
            .map(|(range, _)| (range.end - range.start).as_usize())
            .collect(),
    )
}

#[test]
fn capture_styles_resolve_to_the_most_specific_entry() {
    let styles = capture_styles(
        &[
            ("function", colored(THEME_FUNCTION)),
            ("function.method", colored(THEME_METHOD)),
        ],
        &[("python", &[("function", colored(THEME_PYTHON_FUNCTION))])],
    );

    let cases = [
        ("javascript", "function", Some(THEME_FUNCTION)),
        ("javascript", "function.call", Some(THEME_FUNCTION)),
        ("javascript", "function.method", Some(THEME_METHOD)),
        ("javascript", "function.method.call", Some(THEME_METHOD)),
        ("javascript", "keyword", None),
        // A language entry beats an equally specific global one, but not a more specific one.
        ("python", "function", Some(THEME_PYTHON_FUNCTION)),
        ("python", "function.call", Some(THEME_PYTHON_FUNCTION)),
        ("python", "function.method.call", Some(THEME_METHOD)),
    ];
    for (language, capture, expected) in cases {
        assert_eq!(
            styles
                .resolve(language, capture)
                .and_then(|style| style.color),
            expected,
            "{capture} in {language}",
        );
    }
}

#[test]
fn unknown_captures_and_languages_are_dropped() {
    let styles = capture_styles(
        &[
            ("function", colored(THEME_FUNCTION)),
            ("no.such.capture", colored(THEME_VARIABLE)),
            ("", colored(THEME_VARIABLE)),
        ],
        &[
            (
                "python",
                &[
                    ("function", colored(THEME_PYTHON_FUNCTION)),
                    ("nonsense", colored(THEME_VARIABLE)),
                ],
            ),
            ("klingon", &[("function", colored(THEME_VARIABLE))]),
        ],
    );

    assert_eq!(styles.global.len(), 1);
    assert_eq!(styles.languages.len(), 1);
    assert_eq!(styles.languages["python"].len(), 1);
    assert_eq!(
        styles.resolve("klingon", "function"),
        Some(colored(THEME_FUNCTION)),
    );
}

#[test]
fn palette_falls_back_to_the_color_map() {
    let palette = palette(capture_styles(
        &[
            ("function", colored(THEME_FUNCTION)),
            (
                "keyword",
                CaptureStyle {
                    color: None,
                    underline: true,
                },
            ),
        ],
        &[],
    ));

    let themed = palette
        .highlight("javascript", "function.method")
        .expect("themed capture should be highlighted");
    assert_eq!(themed.color, THEME_FUNCTION);
    assert!(!themed.underline);

    // A style without a color keeps the color map's.
    let underlined = palette
        .highlight("javascript", "keyword")
        .expect("keyword should be highlighted");
    assert_eq!(underlined.color, KEYWORD);
    assert!(underlined.underline);

    let unthemed = palette
        .highlight("javascript", "string")
        .expect("string should be highlighted");
    assert_eq!(unthemed.color, STRING);

    assert!(palette.highlight("javascript", "variable").is_none());
}

#[test]
fn nested_captures_keep_their_color_inside_a_colored_capture() {
    let palette = palette(capture_styles(
        &[
            ("string", colored(THEME_STRING)),
            ("embedded", colored(THEME_EMBEDDED)),
            ("variable", colored(THEME_VARIABLE)),
        ],
        &[],
    ));

    // The substitution is captured as `embedded` along with both of its delimiters, which the
    // color map colors like comments.
    let (colors, underlines) = highlight("javascript", "`a${b + c}d`", palette);
    assert_eq!(
        colors,
        vec![
            (2, THEME_STRING),
            (2, COMMENT),
            (1, THEME_VARIABLE),
            (3, THEME_EMBEDDED),
            (1, THEME_VARIABLE),
            (1, COMMENT),
            (2, THEME_STRING),
        ],
    );
    assert!(underlines.is_empty());
}

#[test]
fn underlined_captures_are_reported_with_their_color() {
    let palette = palette(capture_styles(
        &[
            ("string", colored(THEME_STRING)),
            (
                "variable",
                CaptureStyle {
                    color: Some(THEME_VARIABLE),
                    underline: true,
                },
            ),
        ],
        &[],
    ));

    let (colors, underlines) = highlight("javascript", "`a${b}`", palette);
    assert_eq!(
        colors,
        vec![
            (2, THEME_STRING),
            (2, COMMENT),
            (1, THEME_VARIABLE),
            (1, COMMENT),
            (1, THEME_STRING),
        ],
    );
    assert_eq!(underlines, vec![1]);
}

#[test]
fn highlighting_without_theme_styles_uses_the_color_map() {
    let palette = HighlightPalette {
        fallback: color_map(),
        styles: None,
    };

    let (colors, underlines) = highlight("javascript", "let a = \"b\";", palette);
    assert_eq!(colors, vec![(3, KEYWORD), (3, STRING), (1, COMMENT)]);
    assert!(underlines.is_empty());
}
