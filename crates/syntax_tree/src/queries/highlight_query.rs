use std::collections::HashMap;
use std::iter;
use std::ops::Range;
use std::sync::Arc;

use arborium::tree_sitter::{Node, Query, QueryCursor, TextProvider, Tree};
use languages::{SUPPORTED_LANGUAGES, is_highlight_capture};
use rangemap::RangeMap;
use streaming_iterator::StreamingIterator;
use string_offset::{ByteOffset, CharOffset};
use warp_editor::content::buffer::{Buffer, ToBufferByteOffset, ToBufferCharOffset};
use warp_editor::content::text::Bytes;
use warpui_core::color::ColorU;

/// Color mapping from parsed syntax token name to its corresponding highlighting color.
#[derive(Clone, Copy)]
pub struct ColorMap {
    pub keyword_color: ColorU,
    pub function_color: ColorU,
    pub string_color: ColorU,
    pub type_color: ColorU,
    pub number_color: ColorU,
    pub comment_color: ColorU,
    pub property_color: ColorU,
    pub tag_color: ColorU,
}

/// Style a theme gives the code covered by one highlight capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureStyle {
    /// Left unset to keep the color the capture has in the [`ColorMap`].
    pub color: Option<ColorU>,
    /// Underlines the code in its color, so it has no effect on a capture left without one.
    pub underline: bool,
}

/// Capture styles of a theme, keyed by capture name.
#[derive(Debug)]
pub struct CaptureStyles {
    /// Styles that apply in every language.
    global: HashMap<String, CaptureStyle>,
    /// Styles that apply in one language each, keyed by language name.
    languages: HashMap<String, HashMap<String, CaptureStyle>>,
}

impl CaptureStyles {
    /// Builds the styles from `(capture name, style)` pairs, `global` for every language and
    /// `languages` for one language each. Capture names no highlight query produces and
    /// unsupported languages are dropped.
    pub fn new<G, L, S>(global: G, languages: L) -> Self
    where
        G: IntoIterator<Item = (String, CaptureStyle)>,
        L: IntoIterator<Item = (String, S)>,
        S: IntoIterator<Item = (String, CaptureStyle)>,
    {
        Self {
            global: known_captures(global),
            languages: languages
                .into_iter()
                .filter_map(|(language, styles)| {
                    if SUPPORTED_LANGUAGES.contains(&language.as_str()) {
                        Some((language, known_captures(styles)))
                    } else {
                        log::debug!("Ignoring syntax styles for unsupported language {language}");
                        None
                    }
                })
                .collect(),
        }
    }

    /// Returns the style of the most specific entry for `capture`: the capture itself, or else
    /// the nearest enclosing name, so `function.method.call` falls back to `function.method` and
    /// then `function`. An entry for `language` wins over an equally specific global one.
    fn resolve(&self, language: &str, capture: &str) -> Option<CaptureStyle> {
        let language_styles = self.languages.get(language);
        let mut name = capture;
        loop {
            let style = language_styles
                .and_then(|styles| styles.get(name))
                .or_else(|| self.global.get(name));
            if let Some(style) = style {
                return Some(*style);
            }
            name = &name[..name.rfind('.')?];
        }
    }
}

fn known_captures(
    styles: impl IntoIterator<Item = (String, CaptureStyle)>,
) -> HashMap<String, CaptureStyle> {
    styles
        .into_iter()
        .filter(|(capture, _)| {
            let known = is_highlight_capture(capture);
            if !known {
                log::debug!("Ignoring syntax style for unknown highlight capture {capture}");
            }
            known
        })
        .collect()
}

/// Colors for syntax highlighting: a theme's own capture styles where it has them, and
/// otherwise the color map.
#[derive(Clone)]
pub struct HighlightPalette {
    pub fallback: ColorMap,
    pub styles: Option<Arc<CaptureStyles>>,
}

impl HighlightPalette {
    fn highlight(&self, language: &str, capture: &str) -> Option<Highlight> {
        let style = self
            .styles
            .as_ref()
            .and_then(|styles| styles.resolve(language, capture))
            .unwrap_or_default();
        let color = style
            .color
            .or_else(|| convert_capture_name_to_color(capture, &self.fallback))?;
        Some(Highlight {
            color,
            underline: style.underline,
        })
    }
}

/// How one capture is painted.
#[derive(Clone, Copy)]
struct Highlight {
    color: ColorU,
    underline: bool,
}

/// Highlighting of a range of text, as ranges of uniform style.
#[derive(Default)]
pub struct Highlights {
    pub colors: RangeMap<CharOffset, ColorU>,
    /// Underlined ranges, mapped to the color of the underline.
    pub underlines: RangeMap<CharOffset, ColorU>,
}

impl Highlights {
    fn insert(&mut self, range: Range<CharOffset>, highlight: Highlight) {
        if highlight.underline {
            self.underlines.insert(range.clone(), highlight.color);
        } else {
            self.underlines.remove(range.clone());
        }
        self.colors.insert(range, highlight.color);
    }

    /// Adds every range of `other`, which take precedence where they overlap existing ones.
    pub(crate) fn extend(&mut self, other: Highlights) {
        self.colors.extend(other.colors);
        self.underlines.extend(other.underlines);
    }
}

/// Query for retrieving syntax highlighting information on the tokens.
pub struct HighlightQuery {
    /// Highlight of each capture of the query, indexed like its capture names.
    highlight_map: Vec<Option<Highlight>>,
}

impl HighlightQuery {
    pub fn new(query: &Query, palette: &HighlightPalette, language: &str) -> Self {
        let highlight_map = query
            .capture_names()
            .iter()
            .map(|name| palette.highlight(language, name))
            .collect();

        Self { highlight_map }
    }

    /// Given the a character range, return its corresponding highlights.
    pub fn get_highlighted_chunks(
        &self,
        range: Range<CharOffset>,
        query: &Query,
        buffer: &Buffer,
        tree: &Tree,
    ) -> Highlights {
        let mut highlights = Highlights::default();

        let mut cursor = QueryCursor::new();
        let byte_start = range.start.to_buffer_byte_offset(buffer).as_usize();
        let byte_end = range.end.to_buffer_byte_offset(buffer).as_usize();
        cursor.set_byte_range(byte_start..byte_end);
        let mut captures = cursor.captures(query, tree.root_node(), TextBuffer(buffer));

        // The cursor yields one capture at a time, in document order, along with the match it
        // belongs to. Applying only that capture lets a nested capture keep the style it was
        // given after the capture of the node around it.
        while let Some((query_match, capture_index)) = captures.next() {
            let capture = query_match.captures[*capture_index];
            let Some(highlight) = self
                .highlight_map
                .get(capture.index as usize)
                .and_then(|highlight| *highlight)
            else {
                continue;
            };

            let insertion_range = capture.node.byte_range();
            let char_start = ByteOffset::from(insertion_range.start).to_buffer_char_offset(buffer);
            let char_end = ByteOffset::from(insertion_range.end).to_buffer_char_offset(buffer);
            if char_start < char_end {
                highlights.insert(char_start..char_end, highlight);
            }
        }

        highlights
    }
}

fn convert_capture_name_to_color(name: &str, color_map: &ColorMap) -> Option<ColorU> {
    match name {
        "text.title" => return Some(color_map.keyword_color),
        "text.literal" => return Some(color_map.string_color),
        "text.uri" => return Some(color_map.function_color),
        "text.reference" => return Some(color_map.property_color),
        "constant.numeric" => return Some(color_map.number_color),
        _ => {}
    }
    if name.starts_with("markup.heading") {
        return Some(color_map.keyword_color);
    }
    if name.starts_with("markup.raw") {
        return Some(color_map.string_color);
    }
    match name.split('.').next() {
        Some("keyword") => Some(color_map.keyword_color),
        Some("function") => Some(color_map.function_color),
        Some("string") => Some(color_map.string_color),
        Some("type") => Some(color_map.type_color),
        Some("number") => Some(color_map.number_color),
        Some("comment") => Some(color_map.comment_color),
        Some("property") => Some(color_map.property_color),
        Some("tag") => Some(color_map.tag_color),
        Some("punctuation") => Some(color_map.comment_color),
        // Older capture names that several bundled queries still use for these token classes.
        Some("conditional" | "repeat" | "include" | "exception" | "storageclass") => {
            Some(color_map.keyword_color)
        }
        Some("boolean" | "float") => Some(color_map.number_color),
        Some("method") => Some(color_map.function_color),
        Some("field" | "attribute") => Some(color_map.property_color),
        _ => None,
    }
}

// The default tree-sitter implementation here is unsafe (since the cursor could query invalid ranges outside of content length).
// TODO(kevin): Once we migrate buffer to store ArrayStrings. We should implement the chunks API on buffer directly to avoid collecting
// into a String and then chunking them again for highlighting.
pub struct TextSlice<'a>(pub &'a [u8]);

impl TextSlice<'_> {
    fn get(&self, range: Range<usize>) -> Self {
        Self(self.0.get(range).unwrap_or_default())
    }
}

impl AsRef<[u8]> for TextSlice<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl<'a> TextProvider<TextSlice<'a>> for TextSlice<'a> {
    type I = iter::Once<TextSlice<'a>>;

    fn text(&mut self, node: Node) -> Self::I {
        iter::once(self.get(node.byte_range()))
    }
}

pub struct TextBuffer<'a>(pub &'a Buffer);

impl<'a> TextProvider<&'a [u8]> for TextBuffer<'a> {
    type I = Bytes<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        let range = node.range();
        self.0.bytes_in_range(
            ByteOffset::from(range.start_byte),
            ByteOffset::from(range.end_byte),
        )
    }
}

#[cfg(test)]
#[path = "highlight_query_tests.rs"]
mod tests;
