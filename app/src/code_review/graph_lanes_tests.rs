use super::*;

fn link(from: usize, to: usize, kind: LaneKind) -> Link {
    Link { from, to, kind }
}

#[test]
fn a_lane_that_passes_straight_through_is_one_line_down_the_row() {
    let passing = [link(0, 0, LaneKind::Current)];

    assert_eq!(
        strokes(&passing, &passing),
        [
            Stroke::Vertical {
                lane: 0,
                is_top_half: true,
                kind: LaneKind::Current
            },
            Stroke::Vertical {
                lane: 0,
                is_top_half: false,
                kind: LaneKind::Current
            },
        ]
    );
}

#[test]
fn a_lane_that_joins_a_commit_turns_towards_it_at_the_middle_of_the_row() {
    let joined = strokes(&[link(2, 0, LaneKind::Other)], &[]);

    assert_eq!(
        joined,
        [
            Stroke::Vertical {
                lane: 2,
                is_top_half: true,
                kind: LaneKind::Other
            },
            Stroke::Horizontal {
                from: 0,
                to: 2,
                kind: LaneKind::Other
            },
        ]
    );
}

#[test]
fn a_parent_in_another_lane_is_reached_along_the_middle_and_then_down() {
    let merged = strokes(&[], &[link(0, 1, LaneKind::Other)]);

    assert_eq!(
        merged,
        [
            Stroke::Horizontal {
                from: 0,
                to: 1,
                kind: LaneKind::Other
            },
            Stroke::Vertical {
                lane: 1,
                is_top_half: false,
                kind: LaneKind::Other
            },
        ]
    );
}

#[test]
fn the_current_branch_is_drawn_over_the_lines_it_crosses() {
    let drawn = strokes(
        &[
            link(0, 0, LaneKind::Current),
            link(1, 1, LaneKind::Upstream),
            link(2, 0, LaneKind::Other),
        ],
        &[],
    );

    let kinds: Vec<LaneKind> = drawn.iter().map(Stroke::kind).collect();
    assert_eq!(
        kinds,
        [
            LaneKind::Other,
            LaneKind::Other,
            LaneKind::Upstream,
            LaneKind::Current
        ]
    );
}
