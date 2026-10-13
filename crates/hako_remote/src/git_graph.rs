//! Where the commits of a history go when it is drawn as a graph: one row per commit in the
//! order `git.log` lists them, one lane per line of history that is still open, and the links
//! that join a row to the rows above and below it.

use crate::protocol::GitLogCommit;

/// Which line of history a lane is part of. When lanes meet in a commit, the later of these
/// wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaneKind {
    /// A line that was merged in, or that leads to neither `HEAD` nor its upstream.
    Other,
    /// What only the upstream of the current branch has reached, down to where the two meet.
    Upstream,
    /// The first parents of `HEAD`.
    Current,
}

/// A line across half a row, between a lane at the row's edge and a lane at its middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: usize,
    pub to: usize,
    pub kind: LaneKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane the commit sits in.
    pub column: usize,
    pub kind: LaneKind,
    /// From a lane at the top edge of the row to a lane at its middle.
    pub above: Vec<Link>,
    /// From a lane at the middle of the row to a lane at its bottom edge.
    pub below: Vec<Link>,
    /// The lanes that leave the row at its bottom edge, by position.
    pub after: Vec<LaneKind>,
}

impl GraphRow {
    /// How many lanes wide the row is.
    pub fn width(&self) -> usize {
        let above = self.above.iter().map(|link| link.from.max(link.to) + 1);
        above
            .chain([self.column + 1, self.after.len()])
            .max()
            .unwrap_or(1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lane {
    /// The commit the lane leads down to.
    awaits: String,
    kind: LaneKind,
}

/// The lanes that are open below the last row laid out, which is all that the next row depends
/// on: a history that arrives in pages is laid out page by page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    head: Option<String>,
    upstream: Option<String>,
    lanes: Vec<Lane>,
}

impl Graph {
    /// A graph with no row yet, of a history whose `HEAD` and upstream are at these commits.
    pub fn new(head: Option<&str>, upstream: Option<&str>) -> Self {
        Self {
            head: head.map(str::to_owned),
            upstream: upstream.map(str::to_owned),
            lanes: Vec::new(),
        }
    }

    /// Lays out the commits that follow the ones laid out so far.
    pub fn extend(&mut self, commits: &[GitLogCommit]) -> Vec<GraphRow> {
        commits
            .iter()
            .map(|commit| self.row(&commit.id, &commit.parents))
            .collect()
    }

    /// The kind a lane has for no other reason than the commit it leads to.
    fn kind_of(&self, commit: &str) -> LaneKind {
        if self.head.as_deref() == Some(commit) {
            LaneKind::Current
        } else if self.upstream.as_deref() == Some(commit) {
            LaneKind::Upstream
        } else {
            LaneKind::Other
        }
    }

    /// Lays out the commit that follows the ones laid out so far.
    pub fn row(&mut self, id: &str, parents: &[String]) -> GraphRow {
        let input = std::mem::take(&mut self.lanes);
        let joining: Vec<usize> = input
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.awaits == id)
            .map(|(index, _)| index)
            .collect();
        let column = joining.first().copied().unwrap_or(input.len());
        let kind = joining
            .iter()
            .map(|index| input[*index].kind)
            .chain([self.kind_of(id)])
            .max()
            .unwrap_or(LaneKind::Other);

        let above = input
            .iter()
            .enumerate()
            .map(|(index, lane)| Link {
                from: index,
                to: if lane.awaits == id { column } else { index },
                kind: lane.kind,
            })
            .collect();

        let mut output: Vec<Lane> = Vec::new();
        let mut below = Vec::new();
        let first_parent = parents.first().map(|parent| Lane {
            awaits: parent.clone(),
            kind,
        });
        for (index, lane) in input.into_iter().enumerate() {
            let continued = if index == column {
                first_parent.clone()
            } else if lane.awaits == id {
                None
            } else {
                Some(lane)
            };
            if let Some(lane) = continued {
                below.push(Link {
                    from: index,
                    to: output.len(),
                    kind: lane.kind,
                });
                output.push(lane);
            }
        }
        if joining.is_empty()
            && let Some(lane) = first_parent
        {
            below.push(Link {
                from: column,
                to: output.len(),
                kind: lane.kind,
            });
            output.push(lane);
        }
        for parent in parents.iter().skip(1) {
            let to = match output.iter().position(|lane| &lane.awaits == parent) {
                Some(open) => open,
                None => {
                    output.push(Lane {
                        awaits: parent.clone(),
                        kind: self.kind_of(parent),
                    });
                    output.len() - 1
                }
            };
            below.push(Link {
                from: column,
                to,
                kind: output[to].kind,
            });
        }

        let after = output.iter().map(|lane| lane.kind).collect();
        self.lanes = output;
        GraphRow {
            column,
            kind,
            above,
            below,
            after,
        }
    }
}

#[cfg(test)]
#[path = "git_graph_tests.rs"]
mod tests;
