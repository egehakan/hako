use super::*;

const LETTER: PageGeometry = PageGeometry {
    width: 612.,
    height: 792.,
};
const SLIDE: PageGeometry = PageGeometry {
    width: 400.,
    height: 300.,
};

fn layout() -> PageLayout {
    PageLayout::new(&[LETTER, SLIDE, LETTER])
}

#[test]
fn pages_are_stacked_and_centred() {
    let layout = layout();

    assert_eq!(layout.page_count(), 3);
    assert_eq!(layout.content_size(), Size::new(636., 1932.));
    assert_eq!(layout.page_rect(0), Some(Rect::new(12., 12., 612., 792.)));
    assert_eq!(layout.page_rect(1), Some(Rect::new(118., 816., 400., 300.)));
    assert_eq!(layout.page_rect(2), Some(Rect::new(12., 1128., 612., 792.)));
    assert_eq!(layout.page_rect(3), None);
}

#[test]
fn an_empty_document_has_no_pages_to_show() {
    let layout = PageLayout::new(&[]);
    assert_eq!(layout.page_count(), 0);
    assert_eq!(layout.pages_in(Rect::new(0., 0., 100., 100.)), 0..0);
    assert_eq!(layout.current_page(Rect::new(0., 0., 100., 100.)), 0);
    assert_eq!(layout.scroll_target(0), None);
}

#[test]
fn finds_the_pages_in_view() {
    let layout = layout();
    let view = |top: f32, height: f32| Rect::new(0., top, 636., height);

    assert_eq!(layout.pages_in(view(0., 500.)), 0..1);
    assert_eq!(layout.pages_in(view(700., 200.)), 0..2);
    assert_eq!(layout.pages_in(view(700., 600.)), 0..3);
    assert_eq!(layout.pages_in(view(1200., 400.)), 2..3);
    // Only the gap between the first two pages is in view.
    assert_eq!(layout.pages_in(view(805., 10.)), 1..1);
    assert_eq!(layout.pages_in(view(5000., 100.)), 3..3);
}

#[test]
fn the_current_page_is_the_one_across_the_middle() {
    let layout = layout();
    let view = |top: f32| Rect::new(0., top, 636., 600.);

    assert_eq!(layout.current_page(view(0.)), 0);
    assert_eq!(layout.current_page(view(400.)), 0);
    assert_eq!(layout.current_page(view(600.)), 1);
    assert_eq!(layout.current_page(view(1000.)), 2);
    assert_eq!(layout.current_page(view(9000.)), 2);
}

#[test]
fn going_to_a_page_shows_the_gap_above_it() {
    let layout = layout();
    assert_eq!(layout.scroll_target(0), Some(6.));
    assert_eq!(layout.scroll_target(1), Some(810.));
}

#[test]
fn the_raster_scale_follows_zoom_and_display_up_to_the_texture_limit() {
    assert_eq!(raster_scale(1., 2., 612.), 2.);
    assert_eq!(raster_scale(1.5, 2., 612.), 3.);
    assert_eq!(raster_scale(0.5, 1., 612.), 0.5);
    // 8192 pixels across at most.
    let capped = raster_scale(16., 2., 612.);
    assert!(capped * 612. <= MAX_RASTER_WIDTH + 1.);
    assert!(capped > 13.3 && capped < 13.4);
}

#[test]
fn a_page_is_cut_into_strips_of_bounded_height() {
    let layout = PageLayout::new(&[LETTER]);
    let everything = Rect::new(0., 0., 1e6, 1e6);
    let strips = strips_for_page(&layout, 0, 6., everything);

    // 792 points at 6 pixels per point is 4752 pixels: two full strips and a shorter one.
    assert_eq!(strips.len(), 3);
    assert_eq!(
        strips
            .iter()
            .map(|strip| strip.pixel_top)
            .collect::<Vec<_>>(),
        vec![0, 2048, 4096]
    );
    assert_eq!(
        strips
            .iter()
            .map(|strip| strip.pixel_height)
            .collect::<Vec<_>>(),
        vec![2048, 2048, 656]
    );
    assert!(strips.iter().all(|strip| strip.pixel_width == 3672));
    assert_eq!(strips[0].byte_size(), 3672 * 2048 * 4);

    // Strips tile the page without gaps.
    let page = layout.page_rect(0).unwrap();
    assert_eq!(strips[0].rect.y, page.y);
    for pair in strips.windows(2) {
        assert!((pair[0].rect.bottom() - pair[1].rect.y).abs() < 1e-3);
    }
    assert!((strips[2].rect.bottom() - page.bottom()).abs() < 1e-3);
}

#[test]
fn only_strips_in_view_are_requested() {
    let layout = PageLayout::new(&[LETTER]);
    let top_of_page = Rect::new(0., 0., 636., 200.);
    let strips = strips_for_page(&layout, 0, 6., top_of_page);
    assert_eq!(strips.len(), 1);
    assert_eq!(strips[0].key.strip, 0);

    let middle = Rect::new(0., 340., 636., 30.);
    let strips = strips_for_page(&layout, 0, 6., middle);
    assert_eq!(
        strips
            .iter()
            .map(|strip| strip.key.strip)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    assert!(strips_for_page(&layout, 7, 6., top_of_page).is_empty());
}

#[test]
fn strips_at_different_scales_have_different_keys() {
    let layout = PageLayout::new(&[SLIDE]);
    let everything = Rect::new(0., 0., 1e6, 1e6);
    let at_two = strips_for_page(&layout, 0, 2., everything)[0];
    let at_three = strips_for_page(&layout, 0, 3., everything)[0];

    assert_ne!(at_two.key, at_three.key);
    assert!(at_two.key.is_at_scale(2.));
    assert!(!at_two.key.is_at_scale(3.));
    assert_eq!(
        at_two.key,
        strips_for_page(&layout, 0, 2., everything)[0].key
    );
}

#[test]
fn overscan_grows_the_view_inside_the_content() {
    let content = Size::new(636., 1932.);
    let grown = with_overscan(Rect::new(10., 500., 300., 400.), 0.5, content);
    assert_eq!(grown, Rect::new(0., 300., 636., 800.));

    let at_the_top = with_overscan(Rect::new(0., 0., 636., 400.), 1., content);
    assert_eq!(at_the_top, Rect::new(0., 0., 636., 800.));

    let at_the_bottom = with_overscan(Rect::new(0., 1600., 636., 332.), 1., content);
    assert_eq!(at_the_bottom.bottom(), 1932.);
}
