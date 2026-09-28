//! Packing order: in which order must the goals be filled?
//!
//! Think backward from the solved position. The goal filled LAST must be
//! one whose box can be pulled out to open floor while every other goal is
//! still full. Taking a box off the board only frees space, so once a box
//! can leave it can still leave after other boxes are gone (removability is
//! monotone). Repeatedly removing every box that can leave therefore gives
//! a well-defined layering without any search:
//!
//!   round 1: boxes that can leave the full goal set,
//!   round 2: boxes that can leave once round 1 is gone, ...
//!
//! Reversed, the rounds are the packing layers: layer 0 is filled first.
//! Goals inside one layer are mutually unordered (a partial order, not an
//! arbitrary sequence). Goals whose boxes can never leave form layer 0 —
//! they are filled from the start or the level needs them first.
//!
//! "Leave" means: pulled (player on the outside, other full goals as
//! obstacles) all the way back to a square where some box starts — i.e. a
//! box could have come to this goal from the start position. Stopping at
//! "any square outside the cluster" is wrong: in XSokoban #44 a goal sits
//! above a one-square niche that boxes can only enter FROM that goal, so
//! pulling into the niche is not leaving. This is a relaxation (other boxes
//! are ignored), so the plan is guidance for the search, never a pruning
//! rule.
//!
//! Ordering constraints only arise between goals that crowd each other, so
//! the layering is computed per goal CLUSTER (4-connected goal squares),
//! with the other clusters empty. Measured reason: with one global layering,
//! an independent goal pocket that can be emptied in round 1 lands in the
//! LAST layer, and boxes placed there never count as packed (XSokoban #10).
//!
//! Layers lose information: goals in one layer are not always fillable in
//! any order (XSokoban #44: in a 3x3 goal block, filling the right column
//! first makes the left column unfillable). So for clusters small enough,
//! the plan keeps the exact set of CONSISTENT fillings instead: every subset
//! of the cluster's goals reachable from "all filled" by removing boxes that
//! can leave. From a consistent filling the cluster can still be completed
//! (in the relaxation); packed = the largest consistent subset of the
//! current filling. Layers remain the fallback for clusters whose table
//! would exceed `MAX_TABLE`.

use crate::level::{Board, NONE};
use crate::macros::MacroGen;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;

/// Cap on consistent fillings per cluster table.
const MAX_TABLE: usize = 4_000;

pub struct PackingPlan {
    /// Cluster index per goal square; u16::MAX elsewhere.
    pub cluster: Vec<u16>,
    /// Bit of each goal square within its cluster's mask.
    bit: Vec<u8>,
    /// layer[sq] for goal squares (0 = fill first within its cluster).
    pub layer: Vec<u16>,
    /// Goals per layer, per cluster.
    pub sizes: Vec<Vec<u16>>,
    /// Exact consistency tables, where small enough.
    tables: Vec<Option<ClusterTable>>,
}

/// Consistent fillings of one goal cluster (bit i = i-th goal filled).
struct ClusterTable {
    /// Goals no removal sequence ever empties (set in every consistent
    /// filling): they must all be filled before the table says anything, so
    /// until then each core box earns credit on its own (like layer 0).
    core: u64,
    consistent: FxHashSet<u64>,
    /// Memo: filling -> size of its largest consistent subset.
    best: RefCell<FxHashMap<u64, u32>>,
}

impl ClusterTable {
    /// Retrograde enumeration from the full cluster; None past the cap.
    fn build(
        board: &Board,
        macro_gen: &mut MacroGen,
        flood: &mut Flood,
        full: &mut [bool],
        members: &[u16],
        inside: &impl Fn(u16) -> bool,
    ) -> Option<Self> {
        if members.len() > 63 {
            return None;
        }
        let all = (1u64 << members.len()) - 1;
        let mut consistent = FxHashSet::default();
        consistent.insert(all);
        let mut stack = vec![all];
        while let Some(mask) = stack.pop() {
            for (i, &g) in members.iter().enumerate() {
                full[g as usize] = mask >> i & 1 == 1;
            }
            for (i, &g) in members.iter().enumerate() {
                if mask >> i & 1 == 1 && can_leave(board, macro_gen, flood, full, g, inside) {
                    let next = mask & !(1 << i);
                    if consistent.insert(next) {
                        stack.push(next);
                    }
                }
            }
            if consistent.len() > MAX_TABLE {
                members.iter().for_each(|&g| full[g as usize] = false);
                return None;
            }
        }
        members.iter().for_each(|&g| full[g as usize] = false);
        let core = consistent.iter().fold(all, |acc, m| acc & m);
        Some(ClusterTable { core, consistent, best: RefCell::new(FxHashMap::default()) })
    }

    /// Size of the largest consistent subset of `mask`: drop boxes one at a
    /// time (breadth-first over how many are dropped) until consistent.
    fn packed(&self, mask: u64) -> u32 {
        if mask & self.core != self.core {
            return (mask & self.core).count_ones();
        }
        if self.consistent.contains(&mask) {
            return mask.count_ones();
        }
        if let Some(&v) = self.best.borrow().get(&mask) {
            return v;
        }
        let mut level = vec![mask];
        let mut result = 0;
        'levels: for _ in 0..mask.count_ones() {
            let mut next = FxHashSet::default();
            for &m in &level {
                let mut bits = m;
                while bits != 0 {
                    let b = bits & bits.wrapping_neg();
                    bits &= bits - 1;
                    next.insert(m & !b);
                }
            }
            if let Some(&m) = next.iter().find(|m| self.consistent.contains(m)) {
                result = m.count_ones();
                break 'levels;
            }
            level = next.into_iter().collect();
        }
        self.best.borrow_mut().insert(mask, result);
        result
    }
}

impl PackingPlan {
    pub fn compute(board: &Board) -> Self {
        let n = board.num_squares;
        let mut cluster = vec![u16::MAX; n];
        let mut clusters: Vec<Vec<u16>> = Vec::new();
        for &g in &board.goals {
            if cluster[g as usize] != u16::MAX {
                continue;
            }
            let id = clusters.len() as u16;
            let mut members = vec![g];
            cluster[g as usize] = id;
            let mut i = 0;
            while i < members.len() {
                for &nb in &board.neighbors[members[i] as usize] {
                    if nb != NONE && board.is_goal[nb as usize] && cluster[nb as usize] == u16::MAX {
                        cluster[nb as usize] = id;
                        members.push(nb);
                    }
                }
                i += 1;
            }
            clusters.push(members);
        }

        let mut macro_gen = MacroGen::new(board);
        let mut flood = Flood::new(board);
        let mut layer = vec![u16::MAX; n];
        let mut sizes = Vec::with_capacity(clusters.len());
        let mut tables = Vec::with_capacity(clusters.len());
        let mut full = vec![false; n];
        for (id, members) in clusters.iter().enumerate() {
            let inside = |sq: u16| cluster[sq as usize] == id as u16;
            for &g in members {
                full[g as usize] = true;
            }
            let mut rounds: Vec<Vec<u16>> = Vec::new();
            let mut remaining = members.clone();
            loop {
                let leaving: Vec<u16> = remaining
                    .iter()
                    .copied()
                    .filter(|&g| can_leave(board, &mut macro_gen, &mut flood, &mut full, g, &inside))
                    .collect();
                if leaving.is_empty() {
                    break;
                }
                for &g in &leaving {
                    full[g as usize] = false;
                }
                remaining.retain(|g| !leaving.contains(g));
                rounds.push(leaving);
            }
            for &g in &remaining {
                full[g as usize] = false;
                layer[g as usize] = 0;
            }
            let mut cluster_sizes = vec![remaining.len() as u16];
            for (i, round) in rounds.iter().rev().enumerate() {
                cluster_sizes.push(round.len() as u16);
                for &g in round {
                    layer[g as usize] = i as u16 + 1;
                }
            }
            sizes.push(cluster_sizes);
            tables.push(ClusterTable::build(board, &mut macro_gen, &mut flood, &mut full, members, &inside));
        }
        let mut bit = vec![0u8; n];
        for members in &clusters {
            for (i, &g) in members.iter().enumerate() {
                bit[g as usize] = i as u8;
            }
        }
        PackingPlan { cluster, bit, layer, sizes, tables }
    }

    /// Boxes packed in plan order, summed over clusters: within a cluster,
    /// every box of each complete layer, then the boxes on its first
    /// incomplete layer. Boxes on goals of later layers don't count — they
    /// are in the way of the goals behind them.
    pub fn packed(&self, boxes: &[u16]) -> u32 {
        let mut filled: Vec<Vec<u16>> = self.sizes.iter().map(|s| vec![0; s.len()]).collect();
        let mut masks = vec![0u64; self.sizes.len()];
        for &b in boxes {
            let c = self.cluster[b as usize];
            if c != u16::MAX {
                filled[c as usize][self.layer[b as usize] as usize] += 1;
                masks[c as usize] |= 1 << self.bit[b as usize];
            }
        }
        let mut packed = 0;
        for (c, (cluster_filled, cluster_sizes)) in filled.iter().zip(&self.sizes).enumerate() {
            if let Some(table) = &self.tables[c] {
                packed += table.packed(masks[c]);
                continue;
            }
            for (f, &size) in cluster_filled.iter().zip(cluster_sizes) {
                packed += *f as u32;
                if *f < size {
                    break;
                }
            }
        }
        packed
    }

    /// Per cluster: consistent fillings in its exact table, if it has one.
    pub fn table_sizes(&self) -> Vec<Option<usize>> {
        self.tables.iter().map(|t| t.as_ref().map(|t| t.consistent.len())).collect()
    }

    /// Deepest layer count over clusters (diagnostics).
    pub fn num_layers(&self) -> usize {
        self.sizes.iter().map(|s| s.iter().filter(|&&n| n > 0).count()).max().unwrap_or(0)
    }
}

/// Reusable flood-fill scratch for `can_leave`.
struct Flood {
    stamp: Vec<u32>,
    generation: u32,
    queue: Vec<u16>,
}

impl Flood {
    fn new(board: &Board) -> Self {
        Flood { stamp: vec![0; board.num_squares], generation: 0, queue: Vec::new() }
    }
}

/// Can the box on goal `g` be pulled out of its cluster (`inside`), with
/// the goals marked in `full` occupied, all the way back to a start square?
/// The player may start on any side of `g` whose region reaches a square
/// outside the cluster (it came from outside).
fn can_leave(
    board: &Board,
    macro_gen: &mut MacroGen,
    flood: &mut Flood,
    full: &mut [bool],
    g: u16,
    inside: &impl Fn(u16) -> bool,
) -> bool {
    // One flood per distinct region around g (g itself is occupied).
    let mut sides = [false; 4];
    let mut done = [false; 4];
    for side in 0..4 {
        let start = board.neighbors[g as usize][side];
        if start == NONE || full[start as usize] || done[side] {
            continue;
        }
        flood.generation += 1;
        let f = flood.generation;
        flood.queue.clear();
        flood.queue.push(start);
        flood.stamp[start as usize] = f;
        let mut outside = false;
        let mut i = 0;
        while i < flood.queue.len() {
            let s = flood.queue[i];
            i += 1;
            outside |= !inside(s);
            for &nb in &board.neighbors[s as usize] {
                if nb != NONE && !full[nb as usize] && flood.stamp[nb as usize] != f {
                    flood.stamp[nb as usize] = f;
                    flood.queue.push(nb);
                }
            }
        }
        for other in side..4 {
            let n = board.neighbors[g as usize][other];
            if n != NONE && flood.stamp[n as usize] == f {
                done[other] = true;
                sides[other] = outside;
            }
        }
    }
    if !sides.iter().any(|&s| s) {
        return false;
    }
    macro_gen.pull_search(board, full, g, |side| sides[side], |to| {
        !inside(to) && board.start_boxes.binary_search(&to).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::parse_collection;

    fn plan(text: &str) -> (Board, PackingPlan) {
        let lvl = &parse_collection(text)[0];
        let b = Board::from_level(lvl).unwrap();
        let p = PackingPlan::compute(&b);
        (b, p)
    }

    fn sq(b: &Board, x: usize, y: usize) -> u16 {
        b.sq_index[y * b.width + x]
    }

    #[test]
    fn dead_end_corridor_fills_deepest_first() {
        let (b, p) = plan("#########\n#@$$$...#\n#########");
        let (g5, g6, g7) = (sq(&b, 5, 1), sq(&b, 6, 1), sq(&b, 7, 1));
        assert!(p.layer[g7 as usize] < p.layer[g6 as usize]);
        assert!(p.layer[g6 as usize] < p.layer[g5 as usize]);
        // A box on the entrance goal alone packs nothing; deepest-first does.
        assert_eq!(p.packed(&[g5]), 0);
        assert_eq!(p.packed(&[g7]), 1);
        assert_eq!(p.packed(&[g7, g6]), 2);
    }

    #[test]
    fn open_goals_form_one_layer() {
        let (b, p) = plan("#######\n#     #\n# .$. #\n# $@$ #\n# .$. #\n#     #\n#######");
        assert_eq!(p.num_layers(), 1);
        let goals = b.goals.clone();
        assert_eq!(p.packed(&goals[..2]), 2);
    }

    #[test]
    fn independent_goal_pocket_counts_immediately() {
        // A dead-end goal corridor (right) and a separate goal pocket
        // (left). A box on the pocket's goal must count as packed at once,
        // not only after the whole corridor is full (XSokoban #10 shape).
        let (b, p) = plan("###########\n#.  $@$$...#\n###########");
        let pocket = sq(&b, 1, 1);
        let deepest = sq(&b, 10, 1);
        assert_eq!(p.packed(&[pocket]), 1);
        assert_eq!(p.packed(&[pocket, deepest]), 2);
        assert_eq!(p.packed(&[sq(&b, 8, 1)]), 0, "corridor entrance first packs nothing");
    }
}
