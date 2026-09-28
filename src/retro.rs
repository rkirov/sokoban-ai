//! Relaxed backward plan: a sequence of box moves, including temporary
//! parking, that fills the goals.
//!
//! Why: an order over goals (packing.rs) cannot express PARKING — a box
//! pushed out of a goal area into a niche and brought back later to let the
//! player through (XSokoban #44 fills its 3x3 goal block this way). A plan
//! that says which box goes where, step by step, can.
//!
//! How: solve a relaxed backward problem. Start from the solved position
//! (every goal holds a box) and PULL boxes with macro moves; a box that
//! reaches a square where some box starts is removed (a box could have come
//! from there). Every other box is ignored — that is the relaxation, which
//! makes the problem easy enough to solve in milliseconds. Whenever some box
//! can be removed, only removals are generated (removing a box only makes
//! the rest easier, so this never loses a plan) — that keeps branching tiny.
//!
//! Reversed, the backward path is a forward plan: "bring a box from outside
//! to X", "move the box on X to Y", ... ending with every goal filled. The
//! plan is guidance for the forward search, never a pruning rule.

use crate::level::{Board, NONE};
use crate::macros::{Macro, MacroGen};
use rustc_hash::FxHashSet;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// One forward plan step: a box moves from `from` (None = enters from
/// outside the plan, i.e. from wherever boxes start) to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub from: Option<u16>,
    pub to: u16,
}

pub struct RelaxedPlan {
    pub steps: Vec<Step>,
    /// occupied[p]: squares held by plan boxes after p forward steps
    /// (bitset over squares).
    occupied: Vec<Vec<u64>>,
}

struct Node {
    boxes: Vec<u16>,
    player: u16,
    parent: u32,
    /// Backward move into this node: box from -> to, removed if `gone`.
    from: u16,
    to: u16,
    gone: bool,
}

const NO_PARENT: u32 = u32::MAX;

impl RelaxedPlan {
    /// Search the relaxed backward problem with a node budget. None if no
    /// complete plan was found.
    pub fn compute(board: &Board, max_nodes: usize) -> Option<Self> {
        let n = board.num_squares;
        let mut is_start = vec![false; n];
        for &b in &board.start_boxes {
            is_start[b as usize] = true;
        }
        let mut macro_gen = MacroGen::new(board);
        let mut arena: Vec<Node> = Vec::new();
        let mut seen: FxHashSet<(Vec<u16>, u16)> = FxHashSet::default();
        // Min-heap on (boxes left, pulls so far, insertion order).
        let mut open: BinaryHeap<Reverse<(usize, u32, u32)>> = BinaryHeap::new();
        let mut box_at = vec![false; n];
        let mut reach = vec![false; n];
        let mut moves: Vec<Macro> = Vec::new();

        // Roots: goals filled, one per player region of that board.
        let mut goals = board.goals.clone();
        goals.sort_unstable();
        for &b in &goals {
            box_at[b as usize] = true;
        }
        let mut region_seen = vec![false; n];
        for sq in 0..n as u16 {
            if box_at[sq as usize] || region_seen[sq as usize] {
                continue;
            }
            flood(board, &box_at, sq, &mut region_seen);
            arena.push(Node { boxes: goals.clone(), player: sq, parent: NO_PARENT, from: NONE, to: NONE, gone: false });
            open.push(Reverse((goals.len(), 0, arena.len() as u32 - 1)));
        }
        for &b in &goals {
            box_at[b as usize] = false;
        }

        let mut expanded = 0;
        while let Some(Reverse((left, pulls, idx))) = open.pop() {
            let (boxes, player) = (arena[idx as usize].boxes.clone(), arena[idx as usize].player);
            for &b in &boxes {
                box_at[b as usize] = true;
            }
            reach.iter_mut().for_each(|r| *r = false);
            flood(board, &box_at, player, &mut reach);
            let norm = (0..n).find(|&s| reach[s]).unwrap_or(0) as u16;
            if !seen.insert((boxes.clone(), norm)) {
                boxes.iter().for_each(|&b| box_at[b as usize] = false);
                continue;
            }
            if left == 0 {
                return Some(Self::from_path(board, &arena, idx));
            }
            expanded += 1;
            if expanded > max_nodes {
                return None;
            }

            macro_gen.generate_pulls(board, &boxes, &mut box_at, |s| reach[s as usize], &mut moves);
            boxes.iter().for_each(|&b| box_at[b as usize] = false);
            // Removals first: one per box (its shortest pull to a start square).
            let mut removals: Vec<&Macro> = Vec::new();
            for m in &moves {
                if is_start[m.to as usize] && !removals.iter().any(|r| r.box_idx == m.box_idx) {
                    removals.push(m);
                }
            }
            let children: Vec<(&Macro, bool)> = if removals.is_empty() {
                moves.iter().filter(|m| !board.backward_dead[m.to as usize]).map(|m| (m, false)).collect()
            } else {
                removals.into_iter().map(|m| (m, true)).collect()
            };
            for (m, gone) in children {
                let mut child = boxes.clone();
                if gone {
                    child.remove(m.box_idx);
                } else {
                    child[m.box_idx] = m.to;
                    child.sort_unstable();
                }
                let child_left = child.len();
                arena.push(Node { boxes: child, player: m.player, parent: idx, from: m.from, to: m.to, gone });
                open.push(Reverse((child_left, pulls + 1, arena.len() as u32 - 1)));
            }
        }
        None
    }

    /// Reverse the backward path into forward steps and precompute the
    /// plan-box occupancy after each step.
    fn from_path(board: &Board, arena: &[Node], mut idx: u32) -> Self {
        // Walking from the solved end back to the root visits backward
        // moves in reverse order, i.e. forward order.
        let mut steps = Vec::new();
        while arena[idx as usize].parent != NO_PARENT {
            let node = &arena[idx as usize];
            // Backward: box pulled from -> to (then removed if gone).
            // Forward: box comes from `to` (or from outside) to `from`.
            steps.push(Step { from: if node.gone { None } else { Some(node.to) }, to: node.from });
            idx = node.parent;
        }
        let words = board.num_squares.div_ceil(64);
        let mut occupied = vec![vec![0u64; words]];
        for step in &steps {
            let mut next = occupied.last().unwrap().clone();
            if let Some(f) = step.from {
                next[f as usize / 64] &= !(1u64 << (f % 64));
            }
            next[step.to as usize / 64] |= 1u64 << (step.to % 64);
            occupied.push(next);
        }
        RelaxedPlan { steps, occupied }
    }

    /// Does the plan move an already placed box (parking)? Without parking
    /// it is just one fill order, and packing.rs's partial order is the
    /// better (less restrictive) guide.
    pub fn has_parking(&self) -> bool {
        self.steps.iter().any(|s| s.from.is_some())
    }

    /// Plan progress of a position: the largest p such that every square
    /// the plan's boxes occupy after p steps holds a box.
    pub fn progress(&self, boxes: &[u16]) -> u32 {
        let mut bits = vec![0u64; self.occupied[0].len()];
        for &b in boxes {
            bits[b as usize / 64] |= 1u64 << (b % 64);
        }
        (0..self.occupied.len())
            .rev()
            .find(|&p| self.occupied[p].iter().zip(&bits).all(|(o, b)| o & !b == 0))
            .unwrap_or(0) as u32
    }
}

/// Flood free squares from `start`, setting `mark` (which must be clear
/// for the region).
fn flood(board: &Board, box_at: &[bool], start: u16, mark: &mut [bool]) {
    let mut stack = vec![start];
    mark[start as usize] = true;
    while let Some(s) = stack.pop() {
        for &nb in &board.neighbors[s as usize] {
            if nb != NONE && !mark[nb as usize] && !box_at[nb as usize] {
                mark[nb as usize] = true;
                stack.push(nb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::parse_collection;

    fn board(text: &str) -> Board {
        Board::from_level(&parse_collection(text)[0]).unwrap()
    }

    #[test]
    fn corridor_plan_fills_deepest_first_without_parking() {
        let b = board("#########\n#@$$$...#\n#########");
        let plan = RelaxedPlan::compute(&b, 1000).expect("plan");
        assert!(!plan.has_parking());
        assert_eq!(plan.steps.len(), 3);
        let deepest = b.sq_index[b.width + 7];
        assert_eq!(plan.steps[0].to, deepest, "first placement is the deepest goal");
        assert_eq!(plan.progress(&b.start_boxes), 0);
        assert_eq!(plan.progress(&b.goals), 3);
    }

    #[test]
    fn xsokoban_44_plan_parks_boxes() {
        let text = std::fs::read_to_string("levels/xsokoban.txt").unwrap();
        let lvl = parse_collection(&text).into_iter().find(|l| l.name == "screen.44").unwrap();
        let b = Board::from_level(&lvl).unwrap();
        let plan = RelaxedPlan::compute(&b, 20_000).expect("plan");
        assert!(plan.has_parking(), "the 3x3 goal block needs a parked box");
        assert_eq!(plan.progress(&b.goals), plan.steps.len() as u32);
    }
}
