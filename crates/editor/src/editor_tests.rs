use std::cell::RefCell;

use pathfinder_color::ColorU;
use rangemap::RangeMap;
use string_offset::CharOffset;

use super::TextDecoration;

fn range_map(entries: &[(usize, usize, ColorU)]) -> RangeMap<CharOffset, ColorU> {
    entries
        .iter()
        .map(|(start, end, color)| (CharOffset::from(*start)..CharOffset::from(*end), *color))
        .collect()
}

#[test]
fn base_underlines_are_painted_beneath_override_underlines() {
    let base_color = ColorU::new(1, 0, 0, 255);
    let override_color = ColorU::new(2, 0, 0, 255);
    let base = RefCell::new(range_map(&[(10, 16, base_color), (30, 40, base_color)]));
    let decoration = TextDecoration {
        base_underline_map: Some(base.borrow()),
        underline_range: Some(range_map(&[(14, 18, override_color)])),
        ..Default::default()
    };

    let underlines = decoration.underlines_in(&(CharOffset::from(12)..CharOffset::from(20)));

    assert_eq!(
        underlines.into_iter().collect::<Vec<_>>(),
        vec![(0..2, base_color), (2..6, override_color)],
    );
}

#[test]
fn decorations_without_underlines_paint_none() {
    let decoration = TextDecoration::default();

    let underlines = decoration.underlines_in(&(CharOffset::from(1)..CharOffset::from(9)));

    assert!(underlines.is_empty());
}
