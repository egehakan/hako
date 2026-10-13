use super::*;

/// A history as `(commit, parents)`, newest first.
type History<'a> = &'a [(&'a str, &'a [&'a str])];

fn lay_out(graph: &mut Graph, history: History<'_>) -> Vec<GraphRow> {
    history
        .iter()
        .map(|(id, parents)| {
            let parents: Vec<String> = parents.iter().map(|parent| (*parent).to_owned()).collect();
            graph.row(id, &parents)
        })
        .collect()
}

fn link(from: usize, to: usize, kind: LaneKind) -> Link {
    Link { from, to, kind }
}

fn columns(rows: &[GraphRow]) -> Vec<usize> {
    rows.iter().map(|row| row.column).collect()
}

fn kinds(rows: &[GraphRow]) -> Vec<LaneKind> {
    rows.iter().map(|row| row.kind).collect()
}

#[test]
fn a_linear_history_stays_in_one_lane() {
    let mut graph = Graph::new(Some("c"), None);
    let rows = lay_out(&mut graph, &[("c", &["b"]), ("b", &["a"]), ("a", &[])]);

    assert_eq!(columns(&rows), [0, 0, 0]);
    assert_eq!(kinds(&rows), [LaneKind::Current; 3]);
    assert!(rows[0].above.is_empty());
    assert_eq!(rows[0].below, [link(0, 0, LaneKind::Current)]);
    assert_eq!(rows[1].above, [link(0, 0, LaneKind::Current)]);
    assert_eq!(rows[1].below, [link(0, 0, LaneKind::Current)]);
    // The first commit of the repository has nothing below it.
    assert_eq!(rows[2].above, [link(0, 0, LaneKind::Current)]);
    assert!(rows[2].below.is_empty());
    assert!(rows[2].after.is_empty());
    assert!(rows.iter().all(|row| row.width() == 1));
}

#[test]
fn a_merge_opens_a_lane_that_closes_where_the_branch_began() {
    // `m` merges `f`, which was made on top of `a` while `b` was made on the main line.
    let mut graph = Graph::new(Some("m"), None);
    let rows = lay_out(
        &mut graph,
        &[("m", &["b", "f"]), ("f", &["a"]), ("b", &["a"]), ("a", &[])],
    );

    assert_eq!(columns(&rows), [0, 1, 0, 0]);
    assert_eq!(
        kinds(&rows),
        [
            LaneKind::Current,
            LaneKind::Other,
            LaneKind::Current,
            LaneKind::Current
        ]
    );
    assert_eq!(
        rows[0].below,
        [link(0, 0, LaneKind::Current), link(0, 1, LaneKind::Other)]
    );
    assert_eq!(rows[0].after, [LaneKind::Current, LaneKind::Other]);
    // The main line passes the merged commit by.
    assert_eq!(
        rows[1].above,
        [link(0, 0, LaneKind::Current), link(1, 1, LaneKind::Other)]
    );
    // Both lanes end in the commit they share.
    assert_eq!(
        rows[3].above,
        [link(0, 0, LaneKind::Current), link(1, 0, LaneKind::Other)]
    );
    assert!(rows[3].after.is_empty());
    assert_eq!(rows[1].width(), 2);
    assert_eq!(rows[3].width(), 2);
}

#[test]
fn a_merge_of_a_commit_that_already_has_a_lane_joins_that_lane() {
    // `m` merges `u`, the upstream, which the upstream's own lane is already leading to.
    let mut graph = Graph::new(Some("m"), Some("v"));
    let rows = lay_out(
        &mut graph,
        &[
            ("v", &["u"]),
            ("m", &["b", "u"]),
            ("u", &["a"]),
            ("b", &["a"]),
            ("a", &[]),
        ],
    );

    assert_eq!(columns(&rows), [0, 1, 0, 1, 0]);
    assert_eq!(
        rows[1].below,
        [
            link(0, 0, LaneKind::Upstream),
            link(1, 1, LaneKind::Current),
            link(1, 0, LaneKind::Upstream),
        ]
    );
    assert_eq!(rows[1].after, [LaneKind::Upstream, LaneKind::Current]);
}

#[test]
fn two_open_branches_keep_a_lane_each_and_close_up_when_one_ends() {
    // Three lines are open at once; the middle one ends first, and the last moves over.
    let mut graph = Graph::new(Some("m"), None);
    let rows = lay_out(
        &mut graph,
        &[
            ("m", &["c", "x", "y"]),
            ("x", &["c"]),
            ("y", &["a"]),
            ("c", &["a"]),
            ("a", &[]),
        ],
    );

    assert_eq!(columns(&rows), [0, 1, 2, 0, 0]);
    assert_eq!(
        rows[0].after,
        [LaneKind::Current, LaneKind::Other, LaneKind::Other]
    );
    // `x` hands its lane on to `c`, which the main line leads to as well.
    assert_eq!(rows[1].after.len(), 3);
    // In `c` the lane of `x` ends, and the lane of `y` takes its place.
    assert_eq!(
        rows[3].above,
        [
            link(0, 0, LaneKind::Current),
            link(1, 0, LaneKind::Other),
            link(2, 2, LaneKind::Other),
        ]
    );
    assert_eq!(
        rows[3].below,
        [link(0, 0, LaneKind::Current), link(2, 1, LaneKind::Other)]
    );
    assert_eq!(rows[3].after, [LaneKind::Current, LaneKind::Other]);
    assert_eq!(rows[3].width(), 3);
    assert_eq!(rows[4].above.len(), 2);
}

#[test]
fn an_upstream_that_diverged_runs_beside_the_branch_until_they_meet() {
    // Two commits of ours and one of theirs since `a`.
    let mut graph = Graph::new(Some("l2"), Some("r1"));
    let rows = lay_out(
        &mut graph,
        &[
            ("l2", &["l1"]),
            ("r1", &["a"]),
            ("l1", &["a"]),
            ("a", &["z"]),
            ("z", &[]),
        ],
    );

    assert_eq!(columns(&rows), [0, 1, 0, 0, 0]);
    assert_eq!(
        kinds(&rows),
        [
            LaneKind::Current,
            LaneKind::Upstream,
            LaneKind::Current,
            LaneKind::Current,
            LaneKind::Current
        ]
    );
    assert_eq!(rows[1].after, [LaneKind::Current, LaneKind::Upstream]);
    assert_eq!(
        rows[3].above,
        [
            link(0, 0, LaneKind::Current),
            link(1, 0, LaneKind::Upstream)
        ]
    );
    assert_eq!(rows[3].after, [LaneKind::Current]);
}

#[test]
fn an_upstream_that_is_ahead_leads_into_the_branch() {
    // Nothing of ours is unpushed, and two commits are still to be pulled.
    let mut graph = Graph::new(Some("a"), Some("r2"));
    let rows = lay_out(&mut graph, &[("r2", &["r1"]), ("r1", &["a"]), ("a", &[])]);

    assert_eq!(columns(&rows), [0, 0, 0]);
    assert_eq!(
        kinds(&rows),
        [LaneKind::Upstream, LaneKind::Upstream, LaneKind::Current]
    );
    assert_eq!(rows[2].above, [link(0, 0, LaneKind::Upstream)]);
}

#[test]
fn a_second_page_continues_the_lanes_of_the_first() {
    let history: History<'_> = &[
        ("m", &["b", "f"]),
        ("f", &["e"]),
        ("b", &["a"]),
        ("e", &["a"]),
        ("a", &["z"]),
        ("z", &[]),
    ];
    let mut whole = Graph::new(Some("m"), None);
    let expected = lay_out(&mut whole, history);

    for first_page in 1..history.len() {
        let mut paged = Graph::new(Some("m"), None);
        let mut rows = lay_out(&mut paged, &history[..first_page]);
        rows.extend(lay_out(&mut paged, &history[first_page..]));
        assert_eq!(rows, expected, "paged after {first_page} commits");
    }
    // The merged line is still open where the first page of three commits ends.
    assert_eq!(expected[2].after, [LaneKind::Current, LaneKind::Other]);
    assert_eq!(expected[3].column, 1);
}

#[test]
fn a_commit_that_no_lane_leads_to_starts_a_lane_of_its_own() {
    let mut graph = Graph::new(None, None);
    let rows = lay_out(&mut graph, &[("b", &["a"]), ("d", &["c"])]);

    assert_eq!(columns(&rows), [0, 1]);
    assert_eq!(kinds(&rows), [LaneKind::Other, LaneKind::Other]);
    assert_eq!(rows[1].above, [link(0, 0, LaneKind::Other)]);
    assert_eq!(rows[1].after.len(), 2);
}
