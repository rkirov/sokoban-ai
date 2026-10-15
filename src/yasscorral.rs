//! YASS's corral pruning for the packing-order search (re-derived from
//! YASS 2.153 `CorralPruning`, diff-tested with an instrumented build).
//!
//! A corral is an area the player cannot reach plus the boxes bordering
//! it. Starting from one, every push of one of its boxes is examined
//! (player square N, box target O):
//! - O inside the (combined) corral: fine if the player reaches N now. If N
//!   is another unreachable area, that area is examined as a corral of its
//!   own (recursively). If an outside box stands on N, the push is ignored
//!   when it would freeze the box among corral boxes; otherwise the
//!   blocking box joins the corral (with the boxes chained to it) if it can
//!   step aside, and is ignored if it cannot.
//! - O outside: if the player reaches N, O must be unreachable and is
//!   examined as a corral of its own (else the corral fails: a real
//!   outward push); an unreachable N is examined likewise; a box on N joins
//!   if it can step aside.
//! An examined neighbour that fails fails the whole corral. One that stands
//! alone (needs nothing outside itself) is adopted instead: pruning uses
//! that smaller corral. Otherwise the areas combine. The node then
//! generates only the pushes of the chosen corral's boxes.

use crate::deadlock::FreezeChecker;
use crate::level::{Board, NONE, OPP};

const MAX_DEPTH: usize = 8;

/// Outcome of examining one area.
struct Examined {
    /// It depends on other areas (or has all its boxes on goals), so it
    /// cannot be used on its own.
    combined: bool,
    all_on_goals: bool,
}

pub struct YassCorral {
    /// Stamps: an area's floors get an even stamp t, its boxes t + 1.
    /// Everything above `low` belongs to the corral being built.
    stamp: Vec<u32>,
    time: u32,
    low: u32,
    /// The stamp of the area whose boxes are pruned to (and later ones).
    chosen: u32,
    scratch: Vec<bool>,
}

impl YassCorral {
    pub fn new(board: &Board) -> Self {
        YassCorral { stamp: vec![0; board.num_squares], time: 2, low: 0, chosen: 0, scratch: vec![false; board.num_squares] }
    }

    /// The corral to prune to, if one closes and does not hold every box.
    /// `boxes` gives the order in which boxes (and so seed areas) are tried.
    pub fn analyze(&mut self, board: &Board, box_at: &[bool], reach: &[bool], freeze: &mut FreezeChecker, boxes: &[u16]) -> Option<Vec<(u16, u8)>> {
        if self.time > u32::MAX - 1000 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.time = 2;
        }
        let examined_from = self.time;
        for &b in boxes {
            if !board.neighbors[b as usize].iter().any(|&p| p != NONE && reach[p as usize]) {
                continue; // only corrals next to a reachable box can open now
            }
            for d in [0usize, 2, 1, 3] {
                let seed = board.neighbors[b as usize][d];
                if seed == NONE || box_at[seed as usize] || reach[seed as usize] || self.stamp[seed as usize] > examined_from {
                    continue;
                }
                self.low = self.time + 1;
                self.chosen = self.low;
                let top_own = self.time + 3;
                let Some(top) = self.examine(board, box_at, reach, freeze, seed, 0) else { continue };
                if top.all_on_goals {
                    continue;
                }
                // Unless a nested area was adopted, prune to everything
                // stamped from the top area on.
                self.chosen = self.chosen.max(top_own);
                let corral: Vec<u16> = boxes.iter().copied().filter(|&x| self.stamp[x as usize] >= self.chosen).collect();
                if corral.len() == boxes.len() {
                    return None; // every box: nothing to prune
                }
                let mut pushes = Vec::new();
                for &x in &corral {
                    for e in 0..4 {
                        let to = board.neighbors[x as usize][e];
                        let from = board.neighbors[x as usize][OPP[e]];
                        if to != NONE && from != NONE && !box_at[to as usize] && !board.dead[to as usize] && reach[from as usize] {
                            pushes.push((x, e as u8));
                        }
                    }
                }
                return Some(pushes);
            }
        }
        None
    }

    /// Examine the area from `seed`; None if it fails.
    fn examine(&mut self, board: &Board, box_at: &[bool], reach: &[bool], freeze: &mut FreezeChecker, seed: u16, depth: usize) -> Option<Examined> {
        self.time += 2;
        let t = self.time;
        let own = t + 1;
        let mut all_on_goals = true;
        // Flood the area; boxes bordering it get this area's box stamp.
        let mut boxes: Vec<u16> = Vec::new();
        let mut stack = vec![seed];
        self.stamp[seed as usize] = t;
        while let Some(q) = stack.pop() {
            if board.is_goal[q as usize] {
                all_on_goals = false; // an empty goal inside must be filled
            }
            for &m in &board.neighbors[q as usize] {
                if m == NONE || self.stamp[m as usize] >= t {
                    continue;
                }
                if box_at[m as usize] {
                    self.stamp[m as usize] = own;
                    boxes.push(m);
                } else {
                    self.stamp[m as usize] = t;
                    stack.push(m);
                }
            }
        }
        if depth > MAX_DEPTH {
            return None;
        }
        let mut combined = false;
        let mut i = 0;
        while i < boxes.len() {
            let b = boxes[i];
            i += 1;
            if self.stamp[b as usize] != own {
                continue; // taken over by a nested area
            }
            if !board.is_goal[b as usize] {
                all_on_goals = false;
            }
            for d in 0..4 {
                // The player on n pushes the box to o.
                let n = board.neighbors[b as usize][d];
                let o = board.neighbors[b as usize][OPP[d]];
                if n == NONE || o == NONE || box_at[o as usize] || board.dead[o as usize] || (self.stamp[n as usize] | 1) == own {
                    continue;
                }
                let in_corral = |s: &Self, q: u16| s.stamp[q as usize] > s.low;
                if in_corral(self, o) {
                    if reach[n as usize] {
                        if (self.stamp[o as usize] | 1) != own {
                            combined = true;
                        }
                    } else if in_corral(self, n) {
                        combined = true;
                    } else if !box_at[n as usize] {
                        match self.neighbour(board, box_at, reach, freeze, n, depth)? {
                            Some(c) => {
                                all_on_goals &= c.all_on_goals;
                                combined = true;
                            }
                            None => return Some(Examined { combined: false, all_on_goals: false }),
                        }
                    } else if !self.freezes(board, box_at, b, o, freeze) && self.can_step_aside(board, box_at, n, d, &mut combined, own) {
                        self.add_connected(board, box_at, n, own, &mut boxes);
                    }
                } else if !box_at[n as usize] {
                    let next = if reach[n as usize] {
                        if reach[o as usize] {
                            return None; // a real outward push
                        }
                        o
                    } else if in_corral(self, n) {
                        combined = true;
                        continue;
                    } else {
                        n
                    };
                    match self.neighbour(board, box_at, reach, freeze, next, depth)? {
                        Some(c) => {
                            all_on_goals &= c.all_on_goals;
                            combined = true;
                        }
                        None => return Some(Examined { combined: false, all_on_goals: false }),
                    }
                } else if !in_corral(self, n) {
                    if self.can_step_aside(board, box_at, n, d, &mut combined, own) {
                        self.add_connected(board, box_at, n, own, &mut boxes);
                    }
                } else {
                    combined = true;
                }
            }
        }
        if depth > 0 {
            if all_on_goals {
                combined = true;
            } else if !combined && self.chosen < own {
                self.chosen = own; // stands alone: the candidate to prune to
            }
        }
        Some(Examined { combined, all_on_goals })
    }

    /// Examine a neighbouring area: Err-like None if it fails, Some(None)
    /// if it stands alone (adopted: stop), Some(Some(_)) if it combines.
    fn neighbour(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: &[bool],
        freeze: &mut FreezeChecker,
        seed: u16,
        depth: usize,
    ) -> Option<Option<Examined>> {
        let e = self.examine(board, box_at, reach, freeze, seed, depth + 1)?;
        Some(if e.combined { Some(e) } else { None })
    }

    /// Can the box at `sq` step out of the way across direction `d`? Not
    /// against a wall or a corral box (which ties this corral to it), nor
    /// when both sides are dead squares.
    fn can_step_aside(&self, board: &Board, box_at: &[bool], sq: u16, d: usize, combined: &mut bool, own: u32) -> bool {
        let across = if d < 2 { [2usize, 3] } else { [0, 1] };
        let sides = across.map(|e| board.neighbors[sq as usize][e]);
        if sides.iter().any(|&s| s == NONE) {
            return false;
        }
        for &s in &sides {
            if box_at[s as usize] && self.stamp[s as usize] > self.low {
                if self.stamp[s as usize] != own {
                    *combined = true;
                }
                return false;
            }
        }
        !sides.iter().all(|&s| board.dead[s as usize])
    }

    /// Add the box at `start`, and boxes chained to it that are not in the
    /// corral yet, to this area.
    fn add_connected(&mut self, board: &Board, box_at: &[bool], start: u16, own: u32, boxes: &mut Vec<u16>) {
        let mut stack = vec![start];
        self.stamp[start as usize] = own;
        while let Some(q) = stack.pop() {
            boxes.push(q);
            for &m in &board.neighbors[q as usize] {
                if m != NONE && box_at[m as usize] && self.stamp[m as usize] <= self.low {
                    self.stamp[m as usize] = own;
                    stack.push(m);
                }
            }
        }
    }

    /// Would pushing `b` to `to` freeze it, counting only corral boxes?
    fn freezes(&mut self, board: &Board, box_at: &[bool], b: u16, to: u16, freeze: &mut FreezeChecker) -> bool {
        for q in 0..board.num_squares {
            self.scratch[q] = box_at[q] && self.stamp[q] > self.low;
        }
        self.scratch[b as usize] = false;
        self.scratch[to as usize] = true;
        freeze.is_freeze_deadlock(board, &self.scratch, to)
    }
}
