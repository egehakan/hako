use std::path::Path;

use super::*;

fn table(text: &str) -> Table {
    Table::parse(Arc::from(text), None)
}

fn row(table: &Table, index: usize) -> Vec<String> {
    table.row(index).into_iter().map(Cow::into_owned).collect()
}

#[test]
fn splits_rows_and_fields() {
    let table = table("name,qty\napple,3\npear,12\n");

    assert_eq!(table.delimiter(), b',');
    assert_eq!(table.column_count(), 2);
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.header(), vec!["name", "qty"]);
    assert_eq!(row(&table, 0), vec!["apple", "3"]);
    assert_eq!(row(&table, 1), vec!["pear", "12"]);
    assert!(table.row(2).is_empty());
}

#[test]
fn quoted_fields_keep_delimiters_quotes_and_line_breaks() {
    let table =
        table("id,note\n1,\"a, b\"\n2,\"said \"\"hi\"\"\"\n3,\"line one\nline two\"\n4,plain\n");

    assert_eq!(table.row_count(), 4);
    assert_eq!(row(&table, 0), vec!["1", "a, b"]);
    assert_eq!(row(&table, 1), vec!["2", "said \"hi\""]);
    assert_eq!(row(&table, 2), vec!["3", "line one\nline two"]);
    assert_eq!(row(&table, 3), vec!["4", "plain"]);
}

#[test]
fn unquoted_fields_are_borrowed_from_the_text() {
    let table = table("a,b\n\"x\",y\n\"p\"\"q\",z\n");
    assert!(matches!(table.row(0)[0], Cow::Borrowed("x")));
    assert!(matches!(table.row(0)[1], Cow::Borrowed("y")));
    assert!(matches!(table.row(1)[0], Cow::Owned(_)));
}

#[test]
fn a_byte_order_mark_is_not_part_of_the_first_header() {
    let table = table("\u{feff}name,qty\napple,3\n");
    assert_eq!(table.header(), vec!["name", "qty"]);
}

#[test]
fn windows_line_endings_and_blank_lines() {
    let table = table("a,b\r\n1,2\r\n\r\n3,4");
    assert_eq!(table.row_count(), 2);
    assert_eq!(row(&table, 0), vec!["1", "2"]);
    assert_eq!(row(&table, 1), vec!["3", "4"]);
}

#[test]
fn empty_fields_are_kept() {
    let table = table("a,b,c\n,,\n1,,3\n,2,\n");
    assert_eq!(row(&table, 0), vec!["", "", ""]);
    assert_eq!(row(&table, 1), vec!["1", "", "3"]);
    assert_eq!(row(&table, 2), vec!["", "2", ""]);
}

#[test]
fn ragged_rows_set_the_column_count() {
    let table = table("a,b\n1,2,3,4\n5\n");
    assert_eq!(table.column_count(), 4);
    assert_eq!(row(&table, 1), vec!["5"]);
}

#[test]
fn malformed_quotes_lose_nothing() {
    // A quote that never closes takes the rest of the text into its field.
    let unclosed = table("a,b\n1,\"never closed\n2,3\n");
    assert_eq!(unclosed.row_count(), 1);
    assert_eq!(row(&unclosed, 0), vec!["1", "never closed\n2,3\n"]);

    let text_after_quote = table("a,b\n\"closed\"tail,x\n");
    assert_eq!(row(&text_after_quote, 0), vec!["closedtail", "x"]);
}

#[test]
fn detects_the_delimiter() {
    assert_eq!(detect_delimiter("a;b;c\n1;2;3\n4;5;6\n"), b';');
    assert_eq!(detect_delimiter("a\tb\n1\t2\n"), b'\t');
    assert_eq!(detect_delimiter("a|b|c\n1|2|3\n"), b'|');
    assert_eq!(detect_delimiter("a,b\n1,2\n"), b',');
    assert_eq!(detect_delimiter("just one column\nof text\n"), b',');
    assert_eq!(detect_delimiter(""), b',');
}

#[test]
fn decimal_commas_do_not_hide_a_semicolon_delimiter() {
    // A spreadsheet exported in a locale that writes decimals with a comma.
    let text = "ad;tutar;adet\nelma;1,50;3\narmut;12,25;1\nkiraz;7;2\n";
    assert_eq!(detect_delimiter(text), b';');
    let table = table(text);
    assert_eq!(table.column_count(), 3);
    assert_eq!(row(&table, 0), vec!["elma", "1,50", "3"]);
}

#[test]
fn a_delimiter_inside_quotes_does_not_count() {
    let text = "name;note\n\"a,b,c,d\";x\n\"e,f,g,h\";y\n";
    assert_eq!(detect_delimiter(text), b';');
}

#[test]
fn the_file_name_settles_tab_separated_files() {
    assert_eq!(delimiter_for_path(Path::new("data.TSV")), Some(b'\t'));
    assert_eq!(delimiter_for_path(Path::new("data.csv")), None);

    let table = Table::parse(Arc::from("a, b\tc\n1, 2\t3\n"), Some(b'\t'));
    assert_eq!(table.header(), vec!["a, b", "c"]);
}

#[test]
fn delimiters_have_names() {
    assert_eq!(delimiter_name(b','), "comma");
    assert_eq!(delimiter_name(b';'), "semicolon");
    assert_eq!(delimiter_name(b'\t'), "tab");
}

#[test]
fn an_empty_text_is_an_empty_table() {
    let table = table("");
    assert!(table.is_empty());
    assert_eq!(table.row_count(), 0);
    assert_eq!(table.column_count(), 0);
    assert!(table.header().is_empty());
}

#[test]
fn non_ascii_text_is_measured_in_characters() {
    let table = table("şehir,nüfus\nİstanbul,15\nÇanakkale,0.5\n");
    assert_eq!(row(&table, 0), vec!["İstanbul", "15"]);
    assert_eq!(table.column_chars(), vec![9, 5]);
}

#[test]
fn long_cells_do_not_stretch_a_column_without_bound() {
    let long = "x".repeat(500);
    let table = table(&format!("a,b\n{long},1\n"));
    assert_eq!(table.column_chars(), vec![60, 1]);
}

#[test]
fn column_widths_are_clamped() {
    let widths = column_widths(&[0, 5, 200], 8., 16., 60., 400.);
    assert_eq!(widths, vec![60., 60., 400.]);
    assert_eq!(column_widths(&[20], 8., 16., 60., 400.), vec![176.]);
}

#[test]
fn only_the_columns_in_view_are_drawn() {
    let widths = [100., 100., 100., 100., 100.];

    assert_eq!(visible_columns(&widths, 0., 250.), (0..3, 0.));
    assert_eq!(visible_columns(&widths, 150., 200.), (1..4, 50.));
    assert_eq!(visible_columns(&widths, 200., 100.), (2..3, 0.));
    assert_eq!(visible_columns(&widths, 450., 300.), (4..5, 50.));
    assert_eq!(visible_columns(&widths, 900., 300.), (5..5, 0.));
    assert_eq!(visible_columns(&[], 0., 300.), (0..0, 0.));
}

#[test]
fn horizontal_scroll_range() {
    assert_eq!(max_scroll_x(&[100., 100., 100.], 250.), 50.);
    assert_eq!(max_scroll_x(&[100.], 250.), 0.);
}

#[test]
fn a_large_table_is_indexed_without_copying_its_cells() {
    let mut text = String::from("id,value\n");
    for index in 0..50_000 {
        text.push_str(&format!("{index},\"row {index}\"\n"));
    }
    let table = table(&text);

    assert_eq!(table.row_count(), 50_000);
    assert_eq!(row(&table, 49_999), vec!["49999", "row 49999"]);
}
