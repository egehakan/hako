use futures::executor::block_on;

use super::*;

#[test]
fn formats_byte_counts() {
    assert_eq!(format_bytes(0), "0 bytes");
    assert_eq!(format_bytes(999), "999 bytes");
    assert_eq!(format_bytes(1_000), "1.0 KB");
    assert_eq!(format_bytes(1_440_000), "1.4 MB");
    assert_eq!(format_bytes(250_000_000), "250 MB");
    assert_eq!(format_bytes(3_200_000_000), "3.2 GB");
}

#[test]
fn run_blocking_returns_the_result() {
    assert_eq!(block_on(run_blocking(|| 6 * 7)), Some(42));
}

#[test]
fn run_blocking_survives_a_panic() {
    let result: Option<u32> = block_on(run_blocking(|| panic!("decoder bug")));
    assert_eq!(result, None);
}
