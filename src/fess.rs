//! FESS-lite: feature-space search after Shoham & Schaeffer's FESS algorithm
//! (Festival solver, IEEE CoG 2020), simplified to unit pushes.
//!
//! The search tree lives in domain space; every node projects onto a cell in
//! a small feature space — here 2-D: (boxes on goals, player connectivity =
//! number of player-disconnected regions). The search cycles over active
//! cells, expanding exactly ONE pending move (the least accumulated weight)
//! per cell visit. Moves suggested by advisors (a move that packs a box, a
//! move that improves connectivity) get weight 0, all others weight 1, so
//! accumulated weight counts the "difficult" moves on a line and the search
//! effectively iterates over solutions with 0, 1, 2, ... difficult moves.
//! A child whose features are worse than its parent's cell stays associated
//! with the parent's cell (with its larger weight) instead of spawning an
//! ever-worse cell that would steal expansion cycles.
//!
//! No admissible heuristic is needed; the matching lower bound is used only
//! as an in-cell tie-break and for its deadlock (infeasibility) signal.
//! Deadlock pruning (dead squares, freeze, PI-corral) is shared with the
//! forward A* solver, so completeness within the explored budget matches it.

use crate::corral::{CorralAnalyzer, CorralResult};
use crate::deadlock::FreezeChecker;
use crate::level::{Board, INF, NONE, OPP};
use crate::matching::Matcher;
use crate::solver::{Options, Outcome, Stats};
use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

type Cell = (u8, u8); // (packed boxes, connectivity)

struct FNode {
    boxes: Box<[u16]>,
    #[allow(dead_code)]
    player: u16,
    weight: u32,
    cell: Cell,
    parent: u32,
    push_box_from: u16,
    push_dir: u8,
}

const NO_PARENT: u32 = u32::MAX;

/// Pending move: (weight, matching-h tie-break, seq, parent node, box, dir).
type PendingMove = Reverse<(u32, u32, u64, u32, u16, u8)>;

/// Lexicographic feature ordering: more packed boxes first, then fewer
/// player regions. "Worse" children project onto their parent's cell.
fn better_or_equal(a: Cell, b: Cell) -> bool {
    a.0 > b.0 || (a.0 == b.0 && a.1 <= b.1)
}

pub fn solve(board: &Board, opts: &Options) -> Outcome {
    let start_time = Instant::now();
    let mut stats = Stats::default();

    let mut arena: Vec<FNode> = Vec::new();
    let mut tt: FxHashMap<(Box<[u16]>, u16), ()> = FxHashMap::default();
    let mut cells: FxHashMap<Cell, BinaryHeap<PendingMove>> = FxHashMap::default();
    let mut rotation: Vec<Cell> = Vec::new();
    let mut cursor = 0usize;

    let mut freeze = FreezeChecker::new(board);
    let mut corral = CorralAnalyzer::new(board);
    let mut matcher = Matcher::new();
    let equal_goals_boxes = board.goals.len() == board.start_boxes.len();

    let mut box_at = vec![false; board.num_squares];
    let mut box_list: Vec<u16> = Vec::new();
    let mut reach_stamp = vec![0u32; board.num_squares];
    let mut reach_gen = 0u32;
    let mut bfs_queue: Vec<u16> = Vec::with_capacity(board.num_squares);
    // Separate stamps for connectivity floods so they never clobber the
    // player-reachability stamps consulted by later candidates.
    let mut conn_stamp = vec![0u32; board.num_squares];
    let mut conn_gen = 0u32;
    let mut conn_queue: Vec<u16> = Vec::with_capacity(board.num_squares);
    let mut frozen_flags: Vec<bool> = Vec::new();
    let mut seq = 0u64;

    // Seed with a virtual root entry; the expansion loop materializes it.
    let mut root_heap = BinaryHeap::new();
    root_heap.push(Reverse((0u32, 0u32, 0u64, NO_PARENT, 0u16, 0u8)));
    let root_cell: Cell = (u8::MAX, u8::MAX); // placeholder, replaced on pop
    cells.insert(root_cell, root_heap);
    rotation.push(root_cell);

    let mut pops = 0u64;
    loop {
        if rotation.is_empty() {
            stats.time = start_time.elapsed();
            return Outcome::Unsolvable { stats };
        }
        // Cyclic scan: one move per active cell per round.
        cursor %= rotation.len();
        let cell_key = rotation[cursor];
        let Some(heap) = cells.get_mut(&cell_key) else {
            rotation.swap_remove(cursor);
            continue;
        };
        let Some(Reverse((weight, _, _, parent, box_from, dir))) = heap.pop() else {
            cells.remove(&cell_key);
            rotation.swap_remove(cursor);
            continue;
        };
        cursor += 1;

        pops += 1;
        if stats.expanded >= opts.max_nodes
            || (pops % 256 == 0 && (start_time.elapsed() > opts.time_limit || opts.stopped()))
        {
            stats.time = start_time.elapsed();
            return Outcome::Exhausted { stats };
        }

        // Materialize the child state.
        let (boxes, player, weight) = if parent == NO_PARENT {
            (board.start_boxes.clone().into_boxed_slice(), board.start_player, 0)
        } else {
            let p = &arena[parent as usize];
            let to = board.neighbors[box_from as usize][dir as usize];
            let mut boxes = p.boxes.clone();
            let idx = boxes.iter().position(|&b| b == box_from).unwrap();
            boxes[idx] = to;
            boxes.sort_unstable();
            (boxes, box_from, weight)
        };

        // Player reachability + connectivity (count all regions).
        for &b in box_list.iter() {
            box_at[b as usize] = false;
        }
        box_list.clear();
        box_list.extend_from_slice(&boxes);
        for &b in box_list.iter() {
            box_at[b as usize] = true;
        }
        reach_gen += 1;
        bfs_queue.clear();
        bfs_queue.push(player);
        reach_stamp[player as usize] = reach_gen;
        let mut norm = player;
        let mut head = 0;
        while head < bfs_queue.len() {
            let sq = bfs_queue[head];
            head += 1;
            norm = norm.min(sq);
            for d in 0..4 {
                let n = board.neighbors[sq as usize][d];
                if n != NONE && reach_stamp[n as usize] != reach_gen && !box_at[n as usize] {
                    reach_stamp[n as usize] = reach_gen;
                    bfs_queue.push(n);
                }
            }
        }

        if tt.insert(board.canonical_key(&boxes, &bfs_queue, norm), ()).is_some() {
            stats.duplicates += 1;
            continue;
        }

        // Features of this state.
        let packed = boxes.iter().filter(|&&b| board.is_goal[b as usize]).count() as u8;
        let connectivity =
            count_regions(board, &box_at, &mut conn_stamp, &mut conn_gen, &mut conn_queue);
        let own_cell: Cell = (packed, connectivity);
        let cell = if parent == NO_PARENT {
            own_cell
        } else {
            let pc = arena[parent as usize].cell;
            if better_or_equal(own_cell, pc) { own_cell } else { pc }
        };

        let node_idx = arena.len() as u32;
        arena.push(FNode {
            boxes,
            player,
            weight,
            cell,
            parent,
            push_box_from: box_from,
            push_dir: dir,
        });
        stats.expanded += 1;

        let node = &arena[node_idx as usize];
        if packed as usize == node.boxes.len() {
            stats.time = start_time.elapsed();
            let mut pushes = Vec::new();
            let mut cur = node_idx;
            while arena[cur as usize].parent != NO_PARENT {
                let n = &arena[cur as usize];
                pushes.push((n.push_box_from, n.push_dir));
                cur = n.parent;
            }
            pushes.reverse();
            return Outcome::Solved { pushes, stats };
        }

        // Candidate pushes, restricted by PI-corral analysis when it applies.
        let candidates: Option<Vec<(u16, u8)>> = if opts.corral {
            match corral.analyze(
                board,
                &box_at,
                |sq| reach_stamp[sq as usize] == reach_gen,
                player,
                &mut freeze,
                equal_goals_boxes,
                if parent == NO_PARENT { None } else { Some(player_push_target(board, node)) },
            ) {
                CorralResult::Deadlock => {
                    stats.deadlocks += 1;
                    continue;
                }
                CorralResult::Restrict(p) => Some(p),
                CorralResult::NoPruning => None,
            }
        } else {
            None
        };

        let node_boxes = &node.boxes;
        if crate::deadlock::scan_frozen(&mut freeze, board, &box_at, node_boxes, &mut frozen_flags)
        {
            stats.deadlocks += 1;
            continue;
        }
        let frozen = &frozen_flags;
        let node_cost = |i: usize, j: usize| -> Option<u32> {
            if frozen[i] {
                return (board.goals[j] == node_boxes[i]).then_some(0);
            }
            let d = board.goal_dist[j][node_boxes[i] as usize];
            (d != INF).then_some(d)
        };
        if matcher.solve(node_boxes.len(), board.goals.len(), node_cost).is_none() {
            stats.deadlocks += 1;
            continue;
        }
        matcher.snapshot();

        // Evaluate all legal pushes: legality, deadlocks, feature deltas.
        struct Cand {
            bi: usize,
            dir: u8,
            h: u32,
            packs: bool,
            conn_improves: bool,
            conn_after: u8,
        }
        let mut cands: Vec<Cand> = Vec::new();
        let reach_mark = reach_gen; // player flood stamp
        let mut consider = |bi: usize, d: usize,
                            box_at: &mut Vec<bool>,
                            freeze: &mut FreezeChecker,
                            matcher: &mut Matcher,
                            conn_stamp: &mut Vec<u32>,
                            conn_gen: &mut u32,
                            conn_queue: &mut Vec<u16>,
                            stats: &mut Stats| {
            let b = node_boxes[bi];
            let to = board.neighbors[b as usize][d];
            if to == NONE || box_at[to as usize] || board.dead[to as usize] {
                return;
            }
            let behind = board.neighbors[b as usize][OPP[d]];
            if behind == NONE
                || box_at[behind as usize]
                || reach_stamp[behind as usize] != reach_mark
            {
                return;
            }
            box_at[b as usize] = false;
            box_at[to as usize] = true;
            let mut ok = !freeze.is_freeze_deadlock(board, box_at, to);
            let mut h_child = 0u32;
            if ok {
                let bi_locked = board.is_goal[to as usize] && freeze.frozen(board, box_at, to);
                matcher.restore();
                match matcher.resolve_row(bi, |i, j| {
                    if i == bi {
                        if bi_locked {
                            return (board.goals[j] == to).then_some(0);
                        }
                    } else if frozen[i] {
                        return (board.goals[j] == node_boxes[i]).then_some(0);
                    }
                    let sq = if i == bi { to } else { node_boxes[i] };
                    let dist = board.goal_dist[j][sq as usize];
                    (dist != INF).then_some(dist)
                }) {
                    Some(h) => h_child = h as u32,
                    None => ok = false,
                }
            }
            let mut conn_after = 0u8;
            if ok {
                conn_after = count_regions(board, box_at, conn_stamp, conn_gen, conn_queue);
            }
            box_at[to as usize] = false;
            box_at[b as usize] = true;
            if !ok {
                stats.deadlocks += 1;
                return;
            }
            cands.push(Cand {
                bi,
                dir: d as u8,
                h: h_child,
                packs: board.is_goal[to as usize] && !board.is_goal[b as usize],
                conn_improves: conn_after < connectivity,
                conn_after,
            });
        };
        match &candidates {
            Some(restricted) => {
                for &(b, d) in restricted {
                    let bi = node_boxes.iter().position(|&x| x == b).unwrap();
                    consider(bi, d as usize, &mut box_at, &mut freeze, &mut matcher,
                             &mut conn_stamp, &mut conn_gen, &mut conn_queue, &mut stats);
                }
            }
            None => {
                for bi in 0..node_boxes.len() {
                    for d in 0..4 {
                        consider(bi, d, &mut box_at, &mut freeze, &mut matcher,
                                 &mut conn_stamp, &mut conn_gen, &mut conn_queue, &mut stats);
                    }
                }
            }
        }
        drop(consider);

        // Advisors: the best packing move and the best connectivity-improving
        // move get weight 0; everything else weight 1.
        let packing_pick = cands
            .iter()
            .enumerate()
            .filter(|(_, c)| c.packs)
            .min_by_key(|(_, c)| (c.conn_after, c.h))
            .map(|(i, _)| i);
        let conn_pick = cands
            .iter()
            .enumerate()
            .filter(|(_, c)| c.conn_improves)
            .min_by_key(|(_, c)| (c.conn_after, c.h))
            .map(|(i, _)| i);

        let target_cell = arena[node_idx as usize].cell;
        for (i, c) in cands.iter().enumerate() {
            let mw = if Some(i) == packing_pick || Some(i) == conn_pick { 0 } else { 1 };
            seq += 1;
            stats.generated += 1;
            let entry = Reverse((
                arena[node_idx as usize].weight + mw,
                c.h,
                seq,
                node_idx,
                node_boxes[c.bi],
                c.dir,
            ));
            match cells.entry(target_cell) {
                std::collections::hash_map::Entry::Occupied(mut e) => e.get_mut().push(entry),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(BinaryHeap::from([entry]));
                    rotation.push(target_cell);
                }
            }
        }
    }
}

/// The square the node's incoming push moved its box to.
fn player_push_target(board: &Board, node: &FNode) -> u16 {
    board.neighbors[node.push_box_from as usize][node.push_dir as usize]
}

/// Number of player-connected regions of the free squares.
fn count_regions(
    board: &Board,
    box_at: &[bool],
    stamp: &mut [u32],
    generation: &mut u32,
    queue: &mut Vec<u16>,
) -> u8 {
    *generation += 1;
    let g = *generation;
    let mut regions = 0u8;
    for start in 0..board.num_squares as u16 {
        if box_at[start as usize] || stamp[start as usize] == g {
            continue;
        }
        regions = regions.saturating_add(1);
        queue.clear();
        queue.push(start);
        stamp[start as usize] = g;
        let mut head = 0;
        while head < queue.len() {
            let s = queue[head];
            head += 1;
            for d in 0..4 {
                let n = board.neighbors[s as usize][d];
                if n != NONE && stamp[n as usize] != g && !box_at[n as usize] {
                    stamp[n as usize] = g;
                    queue.push(n);
                }
            }
        }
    }
    regions
}
