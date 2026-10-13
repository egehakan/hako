use super::*;

fn delays_in_seconds(backoff: &mut Backoff, count: usize) -> Vec<u64> {
    (0..count).map(|_| backoff.next_delay().as_secs()).collect()
}

#[test]
fn doubles_from_one_second_up_to_thirty() {
    let mut backoff = Backoff::new();

    assert_eq!(
        delays_in_seconds(&mut backoff, 8),
        [1, 2, 4, 8, 16, 30, 30, 30]
    );
}

#[test]
fn starts_over_after_a_reset() {
    let mut backoff = Backoff::new();
    delays_in_seconds(&mut backoff, 4);

    backoff.reset();

    assert_eq!(delays_in_seconds(&mut backoff, 3), [1, 2, 4]);
}
