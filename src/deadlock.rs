//! Dynamic deadlock detection: freeze deadlocks.
//!
//! A box is *frozen* when it can never be pushed again (or only at the cost
//! of creating a simple deadlock). Following the sokobano.de formulation, a
//! box is blocked along an axis when:
//!   - there is a wall on either side along that axis, or
//!   - there are simple-deadlock squares on both sides (pushing it along this
//!     axis can never be part of a solution), or
//!   - there is an adjacent box on that axis that is blocked along the
//!     *other* axis, checked with the current box treated as a wall. A box
//!     vertically adjacent to another box can only ever move horizontally
//!     while that box remains, so the cross-axis recursion is exact, and the
//!     treat-as-wall trick is sound because a cycle of mutual blocking means
//!     no box in the cycle can be the first to move.
//! A box blocked along both axes is frozen. A frozen box off a goal square is
//! a deadlock.

use crate::level::{Board, NONE};

pub struct FreezeChecker {
    /// Squares treated as walls on the current recursion path.
    on_path: Vec<bool>,
}

/// Scan every box for frozenness. Returns true if the position is dead (a
/// frozen box sits off-goal — possible in root positions, which no push-time
/// cluster check ever covered). Otherwise `flags[i]` marks boxes frozen ON a
/// goal: they can never move again, so in the matching they may only be
/// paired with the goal they occupy (frozen-as-walls, exact rather than
/// merely admissible).
pub fn scan_frozen(
    checker: &mut FreezeChecker,
    board: &Board,
    box_at: &[bool],
    boxes: &[u16],
    flags: &mut Vec<bool>,
) -> bool {
    flags.clear();
    flags.resize(boxes.len(), false);
    for (i, &b) in boxes.iter().enumerate() {
        if checker.frozen(board, box_at, b) {
            if board.is_goal[b as usize] {
                flags[i] = true;
            } else {
                return true;
            }
        }
    }
    false
}

const H_AXIS: usize = 0;
const V_AXIS: usize = 1;
/// Directions per axis: horizontal = {L, R} = {2, 3}, vertical = {U, D} = {0, 1}.
const AXIS_DIRS: [[usize; 2]; 2] = [[2, 3], [0, 1]];

impl FreezeChecker {
    pub fn new(board: &Board) -> Self {
        FreezeChecker { on_path: vec![false; board.num_squares] }
    }

    /// After pushing a box to `pushed`, returns true if the position is a
    /// freeze deadlock. `box_at[sq]` must reflect the position after the push.
    pub fn is_freeze_deadlock(&mut self, board: &Board, box_at: &[bool], pushed: u16) -> bool {
        if !self.frozen(board, box_at, pushed) {
            return false;
        }
        if !board.is_goal[pushed as usize] {
            return true;
        }
        // The pushed box froze on a goal; the freeze may have trapped other
        // boxes off-goal. Test every box in its adjacency cluster.
        let mut stack = vec![pushed];
        let mut seen = vec![pushed];
        while let Some(sq) = stack.pop() {
            for d in 0..4 {
                let n = board.neighbors[sq as usize][d];
                if n != NONE && box_at[n as usize] && !seen.contains(&n) {
                    seen.push(n);
                    stack.push(n);
                }
            }
        }
        seen.iter().any(|&sq| !board.is_goal[sq as usize] && self.frozen(board, box_at, sq))
    }

    /// Whether the box at `sq` can never be pushed again (or only into
    /// simple deadlocks). Public for frozen-as-walls matching: a box frozen
    /// on a goal can only ever satisfy that goal.
    pub fn frozen(&mut self, board: &Board, box_at: &[bool], sq: u16) -> bool {
        self.blocked(board, box_at, sq, H_AXIS) && self.blocked(board, box_at, sq, V_AXIS)
    }

    fn blocked(&mut self, board: &Board, box_at: &[bool], sq: u16, axis: usize) -> bool {
        let [d1, d2] = AXIS_DIRS[axis];
        let n1 = board.neighbors[sq as usize][d1];
        let n2 = board.neighbors[sq as usize][d2];
        // A path-marked square never gets recursed into (it reads as wall
        // here), so each frame marks a previously unmarked square.
        let wall = |n: u16| n == NONE || self.on_path[n as usize];
        if wall(n1) || wall(n2) {
            return true;
        }
        if board.dead[n1 as usize] && board.dead[n2 as usize] {
            return true;
        }
        for n in [n1, n2] {
            if box_at[n as usize] {
                self.on_path[sq as usize] = true;
                let blocked = self.blocked(board, box_at, n, 1 - axis);
                self.on_path[sq as usize] = false;
                if blocked {
                    return true;
                }
            }
        }
        false
    }
}
