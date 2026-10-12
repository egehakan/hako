use super::*;

const VIEWPORT: Size = Size::new(800., 600.);

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn fit_shrinks_large_content_and_leaves_small_content_alone() {
    let fit = ViewTransform::new(ZoomMode::Fit);
    assert_close(fit.scale(Size::new(1600., 600.), VIEWPORT), 0.5);
    assert_close(fit.scale(Size::new(800., 2400.), VIEWPORT), 0.25);
    assert_close(fit.scale(Size::new(64., 64.), VIEWPORT), 1.);
}

#[test]
fn fit_width_also_enlarges() {
    let fit_width = ViewTransform::new(ZoomMode::FitWidth);
    assert_close(fit_width.scale(Size::new(400., 5000.), VIEWPORT), 2.);
    assert_close(fit_width.scale(Size::new(1600., 5000.), VIEWPORT), 0.5);
}

#[test]
fn scale_is_defined_before_the_viewport_is_known() {
    let unknown = Size::default();
    assert_close(
        ViewTransform::new(ZoomMode::Fit).scale(Size::new(10., 10.), unknown),
        1.,
    );
    assert_close(
        ViewTransform::new(ZoomMode::Manual(3.)).scale(Size::new(10., 10.), unknown),
        3.,
    );
}

#[test]
fn small_content_is_centred() {
    let view = ViewTransform::new(ZoomMode::Fit);
    let origin = view.content_origin(Size::new(200., 100.), VIEWPORT);
    assert_close(origin.x, 300.);
    assert_close(origin.y, 250.);
}

#[test]
fn scrolling_stops_at_the_edges() {
    let content = Size::new(1000., 1000.);
    let mut view = ViewTransform::new(ZoomMode::Manual(2.));
    view.scroll_by(Offset::new(-50., -50.), content, VIEWPORT);
    assert_eq!(view.scroll, Offset::new(0., 0.));

    view.scroll_by(Offset::new(5000., 300.), content, VIEWPORT);
    assert_eq!(view.scroll, Offset::new(1200., 300.));
    let origin = view.content_origin(content, VIEWPORT);
    assert_eq!(origin, Offset::new(-1200., -300.));
}

#[test]
fn scrolling_does_nothing_along_an_axis_that_fits() {
    let content = Size::new(400., 3000.);
    let mut view = ViewTransform::new(ZoomMode::Manual(1.));
    view.scroll_by(Offset::new(120., 90.), content, VIEWPORT);
    assert_eq!(view.scroll, Offset::new(0., 90.));
    assert_close(view.content_origin(content, VIEWPORT).x, 200.);
}

#[test]
fn zoom_keeps_the_anchored_point_in_place() {
    let content = Size::new(4000., 3000.);
    let mut view = ViewTransform::new(ZoomMode::Manual(1.));
    view.scroll_by(Offset::new(500., 400.), content, VIEWPORT);
    let anchor = Offset::new(200., 150.);

    let before = view.content_origin(content, VIEWPORT);
    let content_point = Offset::new(anchor.x - before.x, anchor.y - before.y);
    view.zoom_to(2., anchor, content, VIEWPORT);
    let after = view.content_origin(content, VIEWPORT);

    assert_eq!(view.mode, ZoomMode::Manual(2.));
    assert_close(after.x + content_point.x * 2., anchor.x);
    assert_close(after.y + content_point.y * 2., anchor.y);
}

#[test]
fn zoom_from_fit_starts_at_the_fitted_scale() {
    let content = Size::new(1600., 1200.);
    let mut view = ViewTransform::new(ZoomMode::Fit);
    view.zoom_by(2., Offset::new(400., 300.), content, VIEWPORT);
    assert_eq!(view.mode, ZoomMode::Manual(1.));
    // The centre of the content stays in the centre of the viewport.
    assert_eq!(view.scroll, Offset::new(400., 300.));
}

#[test]
fn zoom_is_bounded() {
    let content = Size::new(100., 100.);
    let mut view = ViewTransform::new(ZoomMode::Manual(1.));
    view.zoom_by(1e6, Offset::default(), content, VIEWPORT);
    assert_eq!(view.mode, ZoomMode::Manual(MAX_SCALE));
    view.zoom_by(1e-9, Offset::default(), content, VIEWPORT);
    assert_eq!(view.mode, ZoomMode::Manual(MIN_SCALE));
}

#[test]
fn visible_content_follows_scroll_and_scale() {
    let content = Size::new(1000., 4000.);
    let mut view = ViewTransform::new(ZoomMode::Manual(2.));
    view.scroll_by(Offset::new(100., 1000.), content, VIEWPORT);
    let visible = view.visible_content(content, VIEWPORT);
    assert_close(visible.x, 50.);
    assert_close(visible.y, 500.);
    assert_close(visible.width, 400.);
    assert_close(visible.height, 300.);

    let fitted = ViewTransform::new(ZoomMode::Fit).visible_content(Size::new(80., 60.), VIEWPORT);
    assert_eq!(fitted, Rect::new(0., 0., 80., 60.));
}

#[test]
fn changing_mode_keeps_the_reading_position() {
    let content = Size::new(400., 10_000.);
    let mut view = ViewTransform::new(ZoomMode::FitWidth);
    view.scroll_to_content_y(2500., content, VIEWPORT);
    assert_close(view.scroll.y, 5000.);

    view.set_mode(ZoomMode::Manual(1.), content, VIEWPORT);
    assert_close(view.scroll.y, 2500.);
    assert_close(view.visible_content(content, VIEWPORT).y, 2500.);
}

#[test]
fn zoom_steps_walk_up_and_down() {
    assert_close(next_zoom_step(1., true), 1.25);
    assert_close(next_zoom_step(1., false), 0.75);
    assert_close(next_zoom_step(0.4, true), 0.5);
    assert_close(next_zoom_step(0.4, false), 0.33);
    assert_close(next_zoom_step(1.2501, true), 1.5);
    assert_close(next_zoom_step(32., true), 32.);
    assert_close(next_zoom_step(0.05, false), 0.05);
}

#[test]
fn wheel_zoom_is_symmetric() {
    assert!(wheel_zoom_factor(40.) > 1.);
    assert!(wheel_zoom_factor(-40.) < 1.);
    assert_close(wheel_zoom_factor(40.) * wheel_zoom_factor(-40.), 1.);
    assert_close(wheel_zoom_factor(0.), 1.);
}

#[test]
fn snapping_lands_on_device_pixels() {
    assert_close(snap_to_device_pixels(10.26, 2.), 10.5);
    assert_close(snap_to_device_pixels(10.24, 2.), 10.);
    assert_close(snap_to_device_pixels(10.4, 1.), 10.);
    assert_close(snap_to_device_pixels(10.4, 0.), 10.4);
}

#[test]
fn rect_intersection() {
    let rect = Rect::new(0., 0., 10., 10.);
    assert!(rect.intersects(Rect::new(5., 5., 10., 10.)));
    assert!(!rect.intersects(Rect::new(10., 0., 5., 5.)));
    assert!(!rect.intersects(Rect::new(0., -5., 5., 5.)));
}

#[test]
fn zoom_is_shown_as_a_percentage() {
    assert_eq!(format_zoom(1.), "100%");
    assert_eq!(format_zoom(0.333), "33%");
    assert_eq!(format_zoom(0.05), "5.0%");
}
