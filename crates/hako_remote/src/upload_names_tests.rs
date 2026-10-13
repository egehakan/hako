use super::*;

#[test]
fn takes_one_path_segment_of_at_most_255_bytes() {
    assert!(is_valid("notes.txt"));
    assert!(is_valid(".env"));
    assert!(is_valid("fotoğraf.heic"));
    assert!(is_valid(&"a".repeat(255)));

    assert!(!is_valid(""));
    assert!(!is_valid("."));
    assert!(!is_valid(".."));
    assert!(!is_valid("a/b"));
    assert!(!is_valid("a\0b"));
    assert!(!is_valid(&"a".repeat(256)));
    // 128 two-byte characters.
    assert!(!is_valid(&"ğ".repeat(128)));
}

#[test]
fn numbers_a_taken_name_before_its_last_extension() {
    assert_eq!(numbered("notes.txt", 1), "notes.txt");
    assert_eq!(numbered("notes.txt", 2), "notes (2).txt");
    assert_eq!(numbered("archive.tar.gz", 3), "archive.tar (3).gz");
    assert_eq!(numbered("Makefile", 2), "Makefile (2)");
    assert_eq!(numbered(".env", 2), ".env (2)");
    assert_eq!(numbered("a.", 2), "a (2).");
}

#[test]
fn shortens_the_stem_of_a_numbered_name_that_would_be_too_long() {
    let long = format!("{}.txt", "a".repeat(251));
    let name = numbered(&long, 12);
    assert_eq!(name.len(), MAX_NAME_BYTES);
    assert!(name.ends_with("a (12).txt"));

    let wide = format!("{}.png", "ğ".repeat(126));
    let name = numbered(&wide, 2);
    assert!(name.len() <= MAX_NAME_BYTES);
    assert!(name.ends_with("ğ (2).png"));

    let long_extension = format!("a.{}", "b".repeat(253));
    assert!(numbered(&long_extension, 2).len() <= MAX_NAME_BYTES);
}

#[test]
fn makes_names_for_the_uploads_area_that_a_shell_takes_as_they_are() {
    let name = |sent: &str| for_uploads_area(sent).numbered(1);
    assert_eq!(name("Screen Shot (1).PNG"), "Screen-Shot-1.png");
    assert_eq!(name("fotoğraf.heic"), "foto-raf.heic");
    assert_eq!(name("IMG_0001.JPG"), "IMG_0001.jpg");
    assert_eq!(name("my.photo.v2.jpeg"), "my.photo.v2.jpeg");
    assert_eq!(name("--a  b--.png"), "a-b.png");
    assert_eq!(name(".hidden.png"), "hidden.png");
    assert_eq!(name(".env"), "env");
    assert_eq!(name("$(rm -rf ~).png"), "rm-rf.png");
    assert_eq!(name("'quote'\"; ls.png"), "quote-ls.png");
    assert_eq!(name("日本.png"), "file.png");
    assert_eq!(name("..."), "file");
}

#[test]
fn takes_only_a_short_alphanumeric_extension() {
    let name = |sent: &str| for_uploads_area(sent).numbered(1);
    assert_eq!(name("archive.tar.gz"), "archive.tar.gz");
    assert_eq!(name("data.toolongext"), "data.toolongext");
    assert_eq!(name("data.ab-c"), "data.ab-c");
    assert_eq!(name("photo.ÇPNG"), "photo.-PNG");
    assert_eq!(name("noextension"), "noextension");
}

#[test]
fn keeps_the_first_64_characters_before_the_extension() {
    let sent = format!("{}.png", "a".repeat(100));
    assert_eq!(
        for_uploads_area(&sent).numbered(1),
        format!("{}.png", "a".repeat(64))
    );

    let sent = format!("{}-.....b.png", "a".repeat(62));
    assert_eq!(
        for_uploads_area(&sent).numbered(1),
        format!("{}.png", "a".repeat(62))
    );
}

#[test]
fn numbers_a_taken_name_in_the_uploads_area_with_a_dash() {
    let name = for_uploads_area("Screen Shot (1).PNG");
    assert_eq!(name.numbered(2), "Screen-Shot-1-2.png");
    assert_eq!(name.numbered(3), "Screen-Shot-1-3.png");
    assert_eq!(for_uploads_area("Makefile").numbered(2), "Makefile-2");
}
