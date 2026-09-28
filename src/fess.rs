//! Feature-space search (after Shoham & Schaeffer's FESS, Festival) over
//! macro moves, guided by a packing plan.
//!
//! Why: best-first searches order positions by an estimate of remaining
//! work, which is uninformative for rearrangement puzzles (a correct plan
//! often makes the estimate worse first). FESS instead projects every
//! position onto a small feature space — here (boxes packed in plan order,
//! number of free-space regions) — and cycles over the occupied feature
//! cells, expanding one move per cell per visit. Progress in any feature
//! opens a new cell that gets its own share of effort, so the search
//! cannot drown in one region of the state space.
//!
//! Within a cell, moves are taken by accumulated weight: moves suggested by
//! an advisor (the best move that packs a box, the best move that merges
//! free regions, the best move that reduces the number of boxes standing in
//! other boxes' way — see hotspots.rs) cost 0, others 1. Accumulated weight counts the "unadvised"
//! moves on a line, so lines that follow advice are tried first.
//!
//! A child that is worse than its parent's cell stays in the parent's cell
//! (with its larger weight) instead of opening a worse cell that would take
//! a share of the effort.
//!
//! "Packed" comes from the goal-filling order (packing.rs); for levels whose
//! relaxed backward plan needs parking (a box placed and moved again), the
//! order cannot express that, so progress along that plan (retro.rs) is used
//! instead.
//!
//! Moves are macro moves (one box, any number of pushes; see macros.rs), so
//! the depth of a solution is its number of box moves, not pushes.
//!
//! Completeness: every generated move is eventually expanded unless it
//! leads to a proven deadlock or an already-expanded position, so an empty
//! queue proves the level unsolvable. The plan and advisors only order work.

use crate::corral::{CorralAnalyzer, CorralResult};
use crate::deadlock::FreezeChecker;
use crate::hotspots::Hotspots;
use crate::level::{Board, INF, NONE};
use crate::macros::{Macro, MacroGen, Regions};
use crate::matching::Matcher;
use crate::packing::PackingPlan;
use crate::retro::RelaxedPlan;
use crate::solver::{Options, Outcome, Stats};
use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

/// Feature cell: (packed boxes, free-space regions).
type Cell = (u32, u32);

/// Lexicographic feature order: more packed boxes, then fewer regions.
fn better(a: Cell, b: Cell) -> bool {
    a.0 > b.0 || (a.0 == b.0 && a.1 < b.1)
}

struct Node {
    boxes: Box<[u16]>,
    /// Actual player square (after the move that produced this node).
    player: u16,
    parent: u32,
    /// The move from the parent: index into parent's boxes, destination.
    box_idx: u16,
    to: u16,
    weight: u32,
    cell: Cell,
}

const NO_PARENT: u32 = u32::MAX;

/// Pending move, min-heap order: accumulated weight, then the child's
/// features (more packed, fewer regions), then its distance sum, then
/// newest first.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Pending {
    weight: u32,
    unpacked: u32,
    regions: u32,
    hotspots: u32,
    dist: u32,
    newest: Reverse<u64>,
    parent: u32,
    box_idx: u16,
    to: u16,
    player: u16,
}

/// Per-thread cap on queued moves (~40 bytes each), so long runs degrade to
/// "exhausted" instead of exhausting memory.
const MAX_PENDING: usize = 20_000_000;

pub fn solve(board: &Board, opts: &Options) -> Outcome {
    let start_time = Instant::now();
    let mut stats = Stats::default();
    let plan = PackingPlan::compute(board);
    let relaxed = RelaxedPlan::compute(board, 20_000).filter(|r| r.has_parking());
    let hotspots = Hotspots::compute(board);
    let packed = |boxes: &[u16]| match &relaxed {
        Some(r) => r.progress(boxes),
        None => plan.packed(boxes),
    };

    let mut arena: Vec<Node> = Vec::new();
    let mut expanded: FxHashMap<(Box<[u16]>, u16), ()> = FxHashMap::default();
    let mut cells: FxHashMap<Cell, BinaryHeap<Reverse<Pending>>> = FxHashMap::default();
    let mut rotation: Vec<Cell> = Vec::new();
    let mut cursor = 0usize;
    let mut pending = 0usize;
    // Best cell reached, for FESS_DEBUG diagnostics on give-up.
    let mut best_cell: Cell = (0, u32::MAX);
    let mut seq = 0u64;

    let mut freeze = FreezeChecker::new(board);
    let dead_sets = crate::deadsets::DeadSetTables::new(board, crate::deadsets::Direction::Forward);
    let mut corral = CorralAnalyzer::new(board);
    let mut matcher = Matcher::new();
    let mut macro_gen = MacroGen::new(board);
    let mut regions = RegionCounter::new(board);
    let equal_goals_boxes = board.goals.len() == board.start_boxes.len();

    let mut box_at = vec![false; board.num_squares];
    let mut reach = vec![0u32; board.num_squares];
    let mut reach_gen = 0u32;
    let mut reach_list: Vec<u16> = Vec::new();
    let mut frozen_flags: Vec<bool> = Vec::new();
    let mut frozen_squares: Vec<u16> = Vec::new();
    let mut frozen_walls = crate::deadlock::FrozenWalls::new(board);
    let mut moves: Vec<Macro> = Vec::new();

    // The root is expanded directly; everything after comes off the queues.
    let mut next: Option<Pending> = Some(Pending {
        weight: 0,
        unpacked: 0,
        regions: 0,
        hotspots: 0,
        dist: 0,
        newest: Reverse(0),
        parent: NO_PARENT,
        box_idx: 0,
        to: 0,
        player: board.start_player,
    });

    loop {
        let entry = match next.take() {
            Some(e) => e,
            None => {
                if start_time.elapsed() > opts.time_limit
                    || opts.stopped()
                    || stats.expanded >= opts.max_nodes
                    || pending > MAX_PENDING
                {
                    stats.time = start_time.elapsed();
                    if std::env::var_os("FESS_DEBUG").is_some() {
                        eprintln!(
                            "fess: best cell (packed {}, regions {}) of {} boxes; {} cells",
                            best_cell.0,
                            best_cell.1,
                            board.start_boxes.len(),
                            rotation.len()
                        );
                    }
                    return Outcome::Exhausted { stats };
                }
                // Cyclic scan: one move from the next active cell.
                let Some(e) = pop_cyclic(&mut cells, &mut rotation, &mut cursor) else {
                    stats.time = start_time.elapsed();
                    return Outcome::Unsolvable { stats };
                };
                pending -= 1;
                e
            }
        };

        // Materialize the position.
        let (boxes, weight, parent_cell) = if entry.parent == NO_PARENT {
            (board.start_boxes.clone().into_boxed_slice(), 0, None)
        } else {
            let p = &arena[entry.parent as usize];
            let mut boxes = p.boxes.clone();
            boxes[entry.box_idx as usize] = entry.to;
            boxes.sort_unstable();
            (boxes, entry.weight, Some(p.cell))
        };
        let player = entry.player;
        for &b in &boxes {
            box_at[b as usize] = true;
        }
        reach_gen += 1;
        let norm = flood(board, &box_at, player, &mut reach, reach_gen, &mut reach_list);

        let outcome: Option<Outcome> = 'expand: {
            if expanded.insert(board.canonical_key(&boxes, &reach_list, norm), ()).is_some() {
                stats.duplicates += 1;
                break 'expand None;
            }
            if boxes.iter().all(|&b| board.is_goal[b as usize]) {
                let node_idx = arena.len() as u32;
                arena.push(Node {
                    boxes: boxes.clone(),
                    player,
                    parent: entry.parent,
                    box_idx: entry.box_idx,
                    to: entry.to,
                    weight,
                    cell: (0, 0),
                });
                stats.time = start_time.elapsed();
                let pushes = reconstruct(board, &arena, node_idx, &mut macro_gen);
                break 'expand Some(Outcome::Solved { pushes, stats: stats.clone() });
            }

            // Expensive deadlock checks, only for positions actually chosen.
            let restricted = if opts.corral {
                let focus = (entry.parent != NO_PARENT).then_some(entry.to);
                match corral.analyze(
                    board,
                    &box_at,
                    |sq| reach[sq as usize] == reach_gen,
                    player,
                    &mut freeze,
                    equal_goals_boxes,
                    focus,
                ) {
                    CorralResult::Deadlock => {
                        stats.deadlocks += 1;
                        break 'expand None;
                    }
                    CorralResult::Restrict(p) => Some(p),
                    CorralResult::NoPruning => None,
                }
            } else {
                None
            };
            if crate::deadlock::scan_frozen(&mut freeze, board, &box_at, &boxes, &mut frozen_flags) {
                stats.deadlocks += 1;
                break 'expand None;
            }
            // Every box must still reach a distinct goal, with boxes frozen on
            // goals as walls (see deadlock::FrozenWalls).
            let frozen = &frozen_flags;
            frozen_squares.clear();
            frozen_squares.extend(boxes.iter().zip(frozen.iter()).filter(|p| *p.1).map(|p| *p.0));
            let walled = (!frozen_squares.is_empty()).then(|| frozen_walls.distances(board, &frozen_squares));
            let dist = walled.as_deref().unwrap_or(&board.goal_dist);
            let feasible = matcher.solve(boxes.len(), board.goals.len(), |i, j| {
                if frozen[i] {
                    return (board.goals[j] == boxes[i]).then_some(0);
                }
                let d = dist[j][boxes[i] as usize];
                (d != INF).then_some(d)
            });
            if feasible.is_none() {
                stats.deadlocks += 1;
                break 'expand None;
            }

            // This node's features and cell.
            let own: Cell = (packed(&boxes), regions.label(board, &box_at));
            let cell = match parent_cell {
                Some(pc) if !better(own, pc) => pc,
                _ => own,
            };
            if better(cell, best_cell) {
                best_cell = cell;
            }
            let node_idx = arena.len() as u32;
            arena.push(Node { boxes, player, parent: entry.parent, box_idx: entry.box_idx, to: entry.to, weight, cell });
            stats.expanded += 1;
            let node = &arena[node_idx as usize];

            // Generate and score children (each move's region count comes
            // from the generator's articulation data, see macros.rs).
            let own_regions = Regions { label: &regions.label, count: own.1 };
            match &restricted {
                Some(first) => macro_gen.generate_restricted(board, &node.boxes, &mut box_at, first, Some(&own_regions), &mut moves),
                None => macro_gen.generate(board, &node.boxes, &mut box_at, |sq| reach[sq as usize] == reach_gen, Some(&own_regions), &mut moves),
            }
            let dist_sum: u32 = node.boxes.iter().map(|&b| board.min_goal_dist[b as usize]).sum();
            let own_hot = hotspots.count(&node.boxes);
            let mut children: Vec<(Cell, u32, usize, u32)> = Vec::with_capacity(moves.len());
            let mut child_boxes: Vec<u16> = Vec::with_capacity(node.boxes.len());
            for (mi, m) in moves.iter().enumerate() {
                box_at[m.from as usize] = false;
                box_at[m.to as usize] = true;
                let dead = freeze.is_freeze_deadlock(board, &box_at, m.to)
                    || dead_sets.moved_box_dead(&node.boxes, m.box_idx, m.to, m.player);
                let child_regions = m.regions;
                box_at[m.to as usize] = false;
                box_at[m.from as usize] = true;
                if dead {
                    stats.deadlocks += 1;
                    continue;
                }
                child_boxes.clear();
                child_boxes.extend_from_slice(&node.boxes);
                child_boxes[m.box_idx] = m.to;
                let child_packed = packed(&child_boxes);
                let d = dist_sum - board.min_goal_dist[m.from as usize] + board.min_goal_dist[m.to as usize];
                children.push(((child_packed, child_regions), d, mi, hotspots.count(&child_boxes)));
            }

            // Advisors: the best packing move and the best region-merging
            // move are free; everything else costs 1.
            let rank = |c: &(Cell, u32, usize, u32)| (Reverse(c.0 .0), c.0 .1, c.3, c.1);
            let packer = children.iter().filter(|c| c.0 .0 > own.0).min_by_key(|c| rank(c)).map(|c| c.2);
            let merger = children.iter().filter(|c| c.0 .1 < own.1).min_by_key(|c| rank(c)).map(|c| c.2);
            let unblocker = children
                .iter()
                .filter(|c| c.3 < own_hot && c.0 .0 >= own.0)
                .min_by_key(|c| rank(c))
                .map(|c| c.2);

            let heap = cells.entry(cell).or_insert_with(|| {
                rotation.push(cell);
                cursor = rotation.len() - 1; // a new cell is served next
                BinaryHeap::new()
            });
            for &((packed, child_regions), d, mi, child_hot) in &children {
                let m = &moves[mi];
                let move_weight = if Some(mi) == packer || Some(mi) == merger || Some(mi) == unblocker { 0 } else { 1 };
                seq += 1;
                heap.push(Reverse(Pending {
                    weight: node.weight + move_weight,
                    unpacked: u32::MAX - packed,
                    regions: child_regions,
                    hotspots: child_hot,
                    dist: d,
                    newest: Reverse(seq),
                    parent: node_idx,
                    box_idx: m.box_idx as u16,
                    to: m.to,
                    player: m.player,
                }));
            }
            stats.generated += children.len() as u64;
            pending += children.len();
            None
        };

        box_at.iter_mut().for_each(|b| *b = false);
        if let Some(out) = outcome {
            return out;
        }
    }
}

/// Pop the least move of the cell under the cursor, dropping exhausted
/// cells; None when every cell is empty.
fn pop_cyclic(
    cells: &mut FxHashMap<Cell, BinaryHeap<Reverse<Pending>>>,
    rotation: &mut Vec<Cell>,
    cursor: &mut usize,
) -> Option<Pending> {
    while !rotation.is_empty() {
        *cursor %= rotation.len();
        let cell = rotation[*cursor];
        match cells.get_mut(&cell).and_then(|h| h.pop()) {
            Some(Reverse(e)) => {
                *cursor += 1;
                return Some(e);
            }
            None => {
                cells.remove(&cell);
                rotation.remove(*cursor);
            }
        }
    }
    None
}

/// Player flood fill; stamps `reach` with `stamp`, lists the region in
/// `list`, returns its minimum square.
fn flood(board: &Board, box_at: &[bool], player: u16, reach: &mut [u32], stamp: u32, list: &mut Vec<u16>) -> u16 {
    list.clear();
    list.push(player);
    reach[player as usize] = stamp;
    let mut norm = player;
    let mut head = 0;
    while head < list.len() {
        let sq = list[head];
        head += 1;
        norm = norm.min(sq);
        for &n in &board.neighbors[sq as usize] {
            if n != NONE && reach[n as usize] != stamp && !box_at[n as usize] {
                reach[n as usize] = stamp;
                list.push(n);
            }
        }
    }
    norm
}

/// Labels the connected regions of free (non-box) squares.
struct RegionCounter {
    /// Region id per free square (u32::MAX on boxes), from the last `label`.
    label: Vec<u32>,
    stack: Vec<u16>,
}

impl RegionCounter {
    fn new(board: &Board) -> Self {
        RegionCounter { label: vec![u32::MAX; board.num_squares], stack: Vec::new() }
    }

    /// Label every free square's region; returns the number of regions.
    fn label(&mut self, board: &Board, box_at: &[bool]) -> u32 {
        self.label.iter_mut().for_each(|l| *l = u32::MAX);
        let mut regions = 0;
        for start in 0..board.num_squares {
            if box_at[start] || self.label[start] != u32::MAX {
                continue;
            }
            self.label[start] = regions;
            self.stack.clear();
            self.stack.push(start as u16);
            while let Some(s) = self.stack.pop() {
                for &n in &board.neighbors[s as usize] {
                    if n != NONE && self.label[n as usize] == u32::MAX && !box_at[n as usize] {
                        self.label[n as usize] = regions;
                        self.stack.push(n);
                    }
                }
            }
            regions += 1;
        }
        regions
    }
}

/// Expand the macro path root..node into unit pushes by replaying it.
fn reconstruct(board: &Board, arena: &[Node], node_idx: u32, macro_gen: &mut MacroGen) -> Vec<(u16, u8)> {
    let mut path = Vec::new();
    let mut cur = node_idx;
    while arena[cur as usize].parent != NO_PARENT {
        path.push(cur);
        cur = arena[cur as usize].parent;
    }
    path.reverse();

    let mut pushes = Vec::new();
    let mut box_at = vec![false; board.num_squares];
    let mut reach = vec![0u32; board.num_squares];
    let mut list = Vec::new();
    for (stamp, &idx) in path.iter().enumerate() {
        let node = &arena[idx as usize];
        let parent = &arena[node.parent as usize];
        box_at.iter_mut().for_each(|b| *b = false);
        for &b in parent.boxes.iter() {
            box_at[b as usize] = true;
        }
        let stamp = stamp as u32 + 1;
        flood(board, &box_at, parent.player, &mut reach, stamp, &mut list);
        let m = Macro {
            box_idx: node.box_idx as usize,
            from: parent.boxes[node.box_idx as usize],
            to: node.to,
            player: node.player,
            pushes: 0,
            regions: 0,
        };
        let unit = macro_gen
            .unit_pushes(board, &mut box_at, |sq| reach[sq as usize] == stamp, &m)
            .expect("macro move replays");
        pushes.extend(unit);
    }
    pushes
}
