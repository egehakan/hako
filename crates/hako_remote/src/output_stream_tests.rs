use super::*;

fn chunk(seq: u64, payload: &[u8]) -> OutputChunk {
    OutputChunk {
        seq,
        payload: payload.to_vec(),
    }
}

#[test]
fn joins_what_arrives_between_sends_into_one_chunk() {
    let mut stream = OutputStream::new(100, 1024);
    stream.push(b"ab");
    stream.push(b"cd");

    assert_eq!(stream.end(), 104);
    assert_eq!(stream.take_chunks(1024), [chunk(100, b"abcd")]);
    assert!(!stream.has_output());
    assert!(stream.take_chunks(1024).is_empty());
}

#[test]
fn starts_each_chunk_where_the_one_before_ended() {
    let mut stream = OutputStream::new(0, 1024);
    stream.push(b"abcdefg");
    let first = stream.take_chunks(3);
    stream.push(b"hi");
    let second = stream.take_chunks(3);

    assert_eq!(first, [chunk(0, b"abc"), chunk(3, b"def"), chunk(6, b"g")]);
    assert_eq!(second, [chunk(7, b"hi")]);
}

#[test]
fn leaves_a_gap_when_it_cannot_hold_more() {
    let mut stream = OutputStream::new(0, 4);
    stream.push(b"abc");
    stream.push(b"de");
    stream.push(b"fg");

    // "abcde" was given up, so the next chunk does not start at 0.
    assert_eq!(stream.take_chunks(1024), [chunk(5, b"fg")]);
    assert_eq!(stream.end(), 7);
}

#[test]
fn announces_a_gap_that_nothing_follows() {
    let mut stream = OutputStream::new(10, 1024);
    stream.push(b"abc");
    stream.mark_lost();

    assert!(stream.has_output());
    assert_eq!(stream.take_chunks(1024), [chunk(14, b"")]);
    assert!(stream.take_chunks(1024).is_empty());

    stream.push(b"x");
    assert_eq!(stream.take_chunks(1024), [chunk(14, b"x")]);
}

#[test]
fn counts_output_larger_than_its_limit() {
    let mut stream = OutputStream::new(0, 4);
    stream.push(b"abcdefgh");

    assert_eq!(stream.end(), 8);
    assert_eq!(stream.take_chunks(1024), [chunk(8, b"")]);
}
