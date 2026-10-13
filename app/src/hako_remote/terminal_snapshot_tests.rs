use super::*;

/// A model with one block already in its history.
fn model() -> TerminalModel {
    let mut model = TerminalModel::mock(None, None);
    model.simulate_block("echo first", "first\r\n");
    model
}

fn snapshot_text(model: &TerminalModel) -> String {
    String::from_utf8(terminal_snapshot(model).bytes).expect("a snapshot is text")
}

/// Asserts that `parts` occur in `text` in this order.
fn assert_in_order(text: &str, parts: &[&str]) {
    let mut rest = text;
    for part in parts {
        let Some(position) = rest.find(part) else {
            panic!("{part:?} is missing, or out of order, in {text:?}");
        };
        rest = &rest[position + part.len()..];
    }
}

#[test]
fn writes_finished_blocks_as_lines_from_the_oldest() {
    let mut model = model();
    model.simulate_block("ls", "\x1b[34mdir\x1b[0m  file\r\n");
    model.simulate_block("pwd", "/home\r\n");

    let text = snapshot_text(&model);

    assert_in_order(
        &text,
        &[
            "echo first",
            "first\r\n",
            "ls",
            "\x1b[0;34mdir\x1b[0m  file\r\n",
            "pwd",
            "/home\r\n",
        ],
    );
    assert!(!text.contains("\x1b[?1049h"));
}

#[test]
fn draws_a_running_program_on_a_screen_of_its_own() {
    let mut model = model();
    model.simulate_long_running_block("watch", "\x1b[3;2Hhi");
    let rows = model.block_list().size().rows();

    let text = snapshot_text(&model);

    // The history is scrolled off before the program's rows are written where it put them.
    let screen = format!("{}\x1b[3;1H hi\x1b[3;4H", "\n".repeat(rows));
    assert_in_order(&text, &["first\r\n", "watch", &screen]);
}

#[test]
fn enters_the_alternate_screen_after_the_history_and_before_its_contents() {
    let mut model = model();
    model.simulate_long_running_block("vim", "\x1b[?1049h\x1b[?1h\x1b[2;1Htext\x1b[1;1H");

    let text = snapshot_text(&model);

    assert_in_order(
        &text,
        &[
            "first\r\n",
            "vim",
            "\x1b[?1049h",
            "\x1b[2;1Htext",
            "\x1b[1;1H",
            "\x1b[?1h",
        ],
    );
}
