use super::*;

const TWO_TONE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <rect width="10" height="10" fill="#ff0000"/>
  <rect x="10" width="10" height="10" fill="#0000ff" fill-opacity="0.5"/>
</svg>"##;

#[test]
fn renders_at_the_requested_scale() {
    let raster = render(TWO_TONE, 3.).unwrap();

    assert_eq!(raster.size, Size::new(20., 10.));
    assert_eq!(raster.pixels_per_unit, 3.);
    assert_eq!(raster.image.dimensions(), (60, 30));
    assert_eq!(raster.image.get_pixel(10, 15).0, [255, 0, 0, 255]);
}

#[test]
fn translucent_pixels_have_straight_alpha() {
    let raster = render(TWO_TONE, 1.).unwrap();
    let pixel = raster.image.get_pixel(15, 5).0;

    assert!((126..=129).contains(&pixel[3]), "alpha was {}", pixel[3]);
    assert_eq!(&pixel[..2], &[0, 0]);
    assert!(pixel[2] >= 253, "blue was {}", pixel[2]);
}

#[test]
fn a_view_box_gives_the_size_when_there_is_no_width() {
    let raster = render(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 24"><circle r="5"/></svg>"#,
        1.,
    )
    .unwrap();
    assert_eq!(raster.size, Size::new(48., 24.));
}

#[test]
fn reports_what_cannot_be_parsed() {
    assert!(matches!(
        render("<svg xmlns='http://www.w3.org/2000/svg'><g></svg>", 1.),
        Err(SvgError::Parse(_))
    ));
    assert!(matches!(render("not an svg", 1.), Err(SvgError::Parse(_))));
}

#[test]
fn the_scale_is_lowered_to_fit_a_raster() {
    let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4000" height="1000"/>"#;
    let raster = render(huge, 16.).unwrap();

    assert!(raster.pixels_per_unit < 16.);
    assert!(raster.image.width() <= 8192);
    assert!(raster.image.width() >= 8000);
}

#[test]
fn raster_scales_are_powers_of_two() {
    let size = Size::new(100., 100.);
    assert_eq!(raster_scale_for(0.3, size), 1.);
    assert_eq!(raster_scale_for(1., size), 1.);
    assert_eq!(raster_scale_for(1.1, size), 2.);
    assert_eq!(raster_scale_for(5., size), 8.);
    assert_eq!(raster_scale_for(1000., size), max_pixels_per_unit(size));
}

#[test]
fn raster_limits() {
    assert_eq!(max_pixels_per_unit(Size::new(8192., 10.)), 1.);
    assert!(max_pixels_per_unit(Size::new(8000., 8000.)) < 1.);
    assert_eq!(max_pixels_per_unit(Size::default()), 1.);
}
