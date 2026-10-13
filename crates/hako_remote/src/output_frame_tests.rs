use super::*;

#[test]
fn lays_out_the_frame_as_the_protocol_describes() {
    let frame = OutputFrame {
        session_id: "ab",
        seq: 0x0102_0304_0506_0708,
        payload: b"ls\r\n",
    };

    assert_eq!(
        frame.encode().unwrap(),
        [
            0x01, 2, b'a', b'b', 1, 2, 3, 4, 5, 6, 7, 8, b'l', b's', b'\r', b'\n'
        ]
    );
}

#[test]
fn decodes_what_it_encodes() {
    let frame = OutputFrame {
        session_id: "6f1c2d0e-7b1a-4c58-9d39-0c7f6f1f2a11",
        seq: 4096,
        payload: &[0x1b, b'[', b'0', b'm'],
    };

    let encoded = frame.encode().unwrap();

    assert_eq!(OutputFrame::decode(&encoded), Some(frame));
}

#[test]
fn encodes_a_frame_without_payload() {
    let frame = OutputFrame {
        session_id: "s",
        seq: 0,
        payload: &[],
    };

    assert_eq!(
        frame.encode().unwrap(),
        [0x01, 1, b's', 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn refuses_an_empty_session_id() {
    let frame = OutputFrame {
        session_id: "",
        seq: 0,
        payload: b"x",
    };

    assert_eq!(frame.encode(), Err(SessionIdLengthError(0)));
}

#[test]
fn refuses_a_session_id_over_255_bytes() {
    let session_id = "s".repeat(256);
    let frame = OutputFrame {
        session_id: &session_id,
        seq: 0,
        payload: b"x",
    };

    assert_eq!(frame.encode(), Err(SessionIdLengthError(256)));
}

#[test]
fn does_not_decode_another_frame_type() {
    assert_eq!(
        OutputFrame::decode(&[0x02, 1, b's', 0, 0, 0, 0, 0, 0, 0, 0]),
        None
    );
}

#[test]
fn does_not_decode_a_frame_cut_short_of_its_seq() {
    assert_eq!(OutputFrame::decode(&[0x01, 1, b's', 0, 0, 0]), None);
}
