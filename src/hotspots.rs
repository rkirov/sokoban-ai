//! Hotspots: boxes standing in other boxes' way.
//!
//! Box A is a hotspot if some other box B could reach fewer goals were A's
//! square a wall. Such boxes usually have to move before the boxes behind
//! them can be packed (XSokoban #23: boxes jam the only approach to the
//! goal room). The count of hotspot boxes is a progress signal that the
//! packing count cannot give while preparation moves are needed.
//!
//! Precomputed per level: for each possible blocker square q (not a goal,
//! not dead — a box on a goal is packing's business), redo every goal's
//! relaxed pull search with q walled, and record which squares p lose
//! goals. At runtime a position's hotspot count is O(boxes^2) bit tests.

use crate::level::{Board, INF, NONE};

pub struct Hotspots {
    /// blocked_by[p]: bitset of squares q whose walling reduces the number
    /// of goals a box on p can reach. Empty when the table was skipped.
    blocked_by: Vec<Vec<u64>>,
}

/// Skip the table when its precomputation would exceed this many steps.
const MAX_WORK: usize = 60_000_000;

impl Hotspots {
    pub fn compute(board: &Board) -> Self {
        let n = board.num_squares;
        let work = n * n * board.goals.len();
        if work > MAX_WORK {
            return Hotspots { blocked_by: Vec::new() };
        }
        let words = n.div_ceil(64);
        let reachable = |p: usize| board.goal_dist.iter().filter(|d| d[p] != INF).count();
        let base: Vec<usize> = (0..n).map(reachable).collect();

        let mut blocked_by = vec![vec![0u64; words]; n];
        let mut count = vec![0usize; n];
        let mut seen = vec![false; n];
        let mut queue: Vec<u16> = Vec::with_capacity(n);
        for q in 0..n {
            if board.is_goal[q] || board.dead[q] {
                continue;
            }
            count.iter_mut().for_each(|c| *c = 0);
            for &g in &board.goals {
                // Relaxed pull search from g with q as a wall.
                seen.iter_mut().for_each(|s| *s = false);
                queue.clear();
                queue.push(g);
                seen[g as usize] = true;
                let mut head = 0;
                while head < queue.len() {
                    let s = queue[head];
                    head += 1;
                    count[s as usize] += 1;
                    for d in 0..4 {
                        let next = board.neighbors[s as usize][d];
                        if next == NONE || next as usize == q || seen[next as usize] {
                            continue;
                        }
                        let beyond = board.neighbors[next as usize][d];
                        if beyond == NONE || beyond as usize == q {
                            continue;
                        }
                        seen[next as usize] = true;
                        queue.push(next);
                    }
                }
            }
            for p in 0..n {
                if p != q && !board.dead[p] && count[p] < base[p] {
                    blocked_by[p][q / 64] |= 1 << (q % 64);
                }
            }
        }
        Hotspots { blocked_by }
    }

    /// Number of boxes that block some other box: the union over boxes b
    /// of (squares blocking b) ∩ (box squares), counted.
    pub fn count(&self, boxes: &[u16]) -> u32 {
        if self.blocked_by.is_empty() {
            return 0;
        }
        let words = self.blocked_by[0].len();
        let mut occupied = [0u64; 32];
        let mut blocking = [0u64; 32];
        if words > occupied.len() {
            return 0; // over 2048 squares: feature off
        }
        for &b in boxes {
            occupied[b as usize / 64] |= 1 << (b % 64);
        }
        for &b in boxes {
            for (w, bits) in self.blocked_by[b as usize].iter().enumerate() {
                blocking[w] |= bits & occupied[w];
            }
        }
        blocking[..words].iter().map(|w| w.count_ones()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::parse_collection;

    #[test]
    fn box_in_corridor_blocks_the_one_behind() {
        // Two boxes in a one-wide corridor leading to two goals: the front
        // box blocks the back one (walled, the back box reaches no goal);
        // the back box does not block the front one.
        let lvl = &parse_collection("#########\n#@ $ $ ..#\n#########")[0];
        let b = Board::from_level(lvl).unwrap();
        let h = Hotspots::compute(&b);
        assert_eq!(h.count(&b.start_boxes), 1);
    }
}
