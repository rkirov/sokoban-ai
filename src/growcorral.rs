//! Grown corrals (re-derived from YASS's corral pruning).
//!
//! A corral is an area the player cannot reach plus the boxes around it.
//! The PI rule (corral.rs) says: if every push of a fence box goes into the
//! area and the player can make all of them now, only those pushes need to
//! be generated — the corral must be opened sooner or later, and pushes
//! into it do not disturb play outside. Here a corral that fails the rule
//! is grown instead of given up:
//!
//! - a push whose player square is another unreachable area, or whose
//!   target is one, merges that area (the corral need not be connected);
//! - an outside box standing where the player must stand joins the corral
//!   if it can step aside (its perpendicular neighbours are free), so its
//!   pushes are generated too; if it cannot, that push is impossible and
//!   ignored, as is a push that would freeze the box among corral boxes.
//!
//! The corral fails only on a push from a reachable square into reachable
//! floor (a real outward push). The result restricts the node to the
//! pushes of the corral's boxes that the player can make now.

use crate::deadlock::FreezeChecker;
use crate::level::{Board, NONE, OPP};

pub struct GrowCorral {
    area: Vec<u32>,
    in_corral: Vec<u32>,
    stamp: u32,
    seen: Vec<u32>,
    seen_stamp: u32,
    corral_box: Vec<bool>,
}

impl GrowCorral {
    pub fn new(board: &Board) -> Self {
        let n = board.num_squares;
        GrowCorral { area: vec![0; n], in_corral: vec![0; n], stamp: 0, seen: vec![0; n], seen_stamp: 0, corral_box: vec![false; n] }
    }

    /// The (box, direction) pushes of the first corral (seeds in square
    /// order) that grows to closure and is unfinished, if any.
    pub fn analyze(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: &[bool],
        freeze: &mut FreezeChecker,
        equal_goals_boxes: bool,
    ) -> Option<Vec<(u16, u8)>> {
        self.seen_stamp += 1;
        for seed in 0..board.num_squares as u16 {
            if box_at[seed as usize] || reach[seed as usize] || self.seen[seed as usize] == self.seen_stamp {
                continue;
            }
            // Only areas next to a box the player can reach can be opened now.
            let opens = board.neighbors[seed as usize].iter().any(|&b| {
                b != NONE && box_at[b as usize] && board.neighbors[b as usize].iter().any(|&p| p != NONE && reach[p as usize])
            });
            if !opens {
                continue;
            }
            if let Some(g) = self.grow(board, box_at, reach, freeze, equal_goals_boxes, seed) {
                return Some(g);
            }
        }
        None
    }

    fn grow(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: &[bool],
        freeze: &mut FreezeChecker,
        equal_goals_boxes: bool,
        seed: u16,
    ) -> Option<Vec<(u16, u8)>> {
        self.stamp += 1;
        let st = self.stamp;
        let mut squares: Vec<u16> = Vec::new();
        let mut boxes: Vec<u16> = Vec::new();
        self.merge_area(board, box_at, reach, seed, &mut squares, &mut boxes);
        // Process boxes until closure; merges and added boxes extend the
        // lists, so earlier boxes are re-checked at the end of a pass.
        loop {
            let mut grew = false;
            let mut i = 0;
            while i < boxes.len() {
                let b = boxes[i];
                i += 1;
                for d in 0..4 {
                    let to = board.neighbors[b as usize][d];
                    let from = board.neighbors[b as usize][OPP[d]];
                    if to == NONE || from == NONE || box_at[to as usize] || board.dead[to as usize] {
                        continue;
                    }
                    if self.area[from as usize] == st || self.in_corral[from as usize] == st {
                        continue; // made from inside the corral
                    }
                    if !box_at[from as usize] {
                        if !reach[from as usize] {
                            self.merge_area(board, box_at, reach, from, &mut squares, &mut boxes);
                            grew = true;
                        } else if self.area[to as usize] != st {
                            if reach[to as usize] {
                                return None; // a real outward push
                            }
                            self.merge_area(board, box_at, reach, to, &mut squares, &mut boxes);
                            grew = true;
                        }
                    } else if !self.freezes(board, &boxes, b, to, freeze) && can_step_aside(board, box_at, &self.in_corral, st, from, d) {
                        // An outside box blocks the player's square; it may
                        // move away first, so it (and its neighbours) join.
                        self.add_connected(board, box_at, from, &mut boxes);
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        let unfinished = boxes.iter().any(|&b| !board.is_goal[b as usize])
            || (equal_goals_boxes && squares.iter().any(|&s| board.is_goal[s as usize]));
        let mut pushes = Vec::new();
        if unfinished {
            for &b in &boxes {
                for d in 0..4 {
                    let to = board.neighbors[b as usize][d];
                    let from = board.neighbors[b as usize][OPP[d]];
                    if to != NONE && from != NONE && !box_at[to as usize] && !board.dead[to as usize] && reach[from as usize] {
                        pushes.push((b, d as u8));
                    }
                }
            }
        }
        (!pushes.is_empty()).then_some(pushes)
    }

    /// Flood the unreachable area from `start` into the corral; boxes
    /// bordering it join too.
    fn merge_area(&mut self, board: &Board, box_at: &[bool], reach: &[bool], start: u16, squares: &mut Vec<u16>, boxes: &mut Vec<u16>) {
        let st = self.stamp;
        if self.area[start as usize] == st {
            return;
        }
        self.area[start as usize] = st;
        self.seen[start as usize] = self.seen_stamp;
        let mut stack = vec![start];
        while let Some(s) = stack.pop() {
            squares.push(s);
            for &n in &board.neighbors[s as usize] {
                if n == NONE {
                    continue;
                }
                if box_at[n as usize] {
                    if self.in_corral[n as usize] != st {
                        self.in_corral[n as usize] = st;
                        boxes.push(n);
                    }
                } else if !reach[n as usize] && self.area[n as usize] != st {
                    self.area[n as usize] = st;
                    self.seen[n as usize] = self.seen_stamp;
                    stack.push(n);
                }
            }
        }
    }

    /// Add the box at `start` and every box chained to it by adjacency.
    fn add_connected(&mut self, board: &Board, box_at: &[bool], start: u16, boxes: &mut Vec<u16>) {
        let st = self.stamp;
        let mut stack = vec![start];
        self.in_corral[start as usize] = st;
        while let Some(s) = stack.pop() {
            boxes.push(s);
            for &n in &board.neighbors[s as usize] {
                if n != NONE && box_at[n as usize] && self.in_corral[n as usize] != st {
                    self.in_corral[n as usize] = st;
                    stack.push(n);
                }
            }
        }
    }

    /// Would pushing corral box `b` to `to` freeze it, counting only the
    /// corral's boxes?
    fn freezes(&mut self, board: &Board, boxes: &[u16], b: u16, to: u16, freeze: &mut FreezeChecker) -> bool {
        for &x in boxes {
            self.corral_box[x as usize] = true;
        }
        self.corral_box[b as usize] = false;
        self.corral_box[to as usize] = true;
        let frozen = freeze.is_freeze_deadlock(board, &self.corral_box, to);
        self.corral_box[to as usize] = false;
        for &x in boxes {
            self.corral_box[x as usize] = false;
        }
        frozen
    }
}

/// Can the box at `sq` move along the axis perpendicular to `d` (out of the
/// pushing player's way)? Not if a wall or a corral box is beside it, or if
/// both sides are dead squares.
fn can_step_aside(board: &Board, box_at: &[bool], in_corral: &[u32], st: u32, sq: u16, d: usize) -> bool {
    let (a, b) = if d < 2 { (2, 3) } else { (0, 1) };
    let s1 = board.neighbors[sq as usize][a];
    let s2 = board.neighbors[sq as usize][b];
    if s1 == NONE || s2 == NONE {
        return false;
    }
    if (box_at[s1 as usize] && in_corral[s1 as usize] == st) || (box_at[s2 as usize] && in_corral[s2 as usize] == st) {
        return false;
    }
    !(board.dead[s1 as usize] && board.dead[s2 as usize])
}
