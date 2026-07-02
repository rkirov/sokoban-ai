//! Bidirectional meet-in-the-middle search.
//!
//! A forward best-first search over pushes and a backward best-first search
//! over pulls run interleaved in one thread, each with its own transposition
//! table keyed identically: (sorted boxes, minimum square of the player's
//! reachable region). Whenever one side materializes a new state it probes
//! the other side's table; equal keys mean equal box sets AND the same
//! player region, so the backward node's pull suffix (reversed into forward
//! pushes) is executable from the forward node's position — the two half
//! paths splice into a full solution of roughly half the search depth per
//! side. Solutions are not push-optimal (first meet wins).
//!
//! Frontier *merging* is a known trap in Sokoban; this is probe-only.
//! Requires #goals == #boxes (the backward start state must be unambiguous);
//! otherwise returns Exhausted immediately so other portfolio strategies
//! cover the level. Forward-side pruning (dead squares, freeze, PI-corral,
//! matching) never blocks a meet: pruned states cannot reach the goal, so
//! the backward space never contains them, and PI-corral only postpones
//! moves along equivalent solutions. Either side exhausting its complete
//! space proves the level unsolvable.

use crate::corral::{CorralAnalyzer, CorralResult};
use crate::deadlock::FreezeChecker;
use crate::level::{Board, INF, NONE, OPP};
use crate::matching::Matcher;
use crate::solver::{Mode, Options, Outcome, Stats};
use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

const NO_PARENT: u32 = u32::MAX;

struct BNode {
    boxes: Box<[u16]>,
    g: u32,
    parent: u32,
    push_box_from: u16,
    push_dir: u8,
}

/// (f, h, seq, parent, box_from, dir) — min-heap on f, then h, then newest.
type Entry = Reverse<(u64, u32, u64, u32, u16, u8)>;
type Key = (Box<[u16]>, u16);

struct Side {
    arena: Vec<BNode>,
    tt: FxHashMap<Key, u32>,
    open: BinaryHeap<Entry>,
    matcher: Matcher,
    seq: u64,
}

impl Side {
    fn new() -> Self {
        Side {
            arena: Vec::new(),
            tt: FxHashMap::default(),
            open: BinaryHeap::new(),
            matcher: Matcher::new(),
            seq: 0,
        }
    }
}

pub fn solve(board: &Board, opts: &Options) -> Outcome {
    let start_time = Instant::now();
    let mut stats = Stats::default();

    if board.goals.len() != board.start_boxes.len() {
        stats.time = start_time.elapsed();
        return Outcome::Exhausted { stats };
    }

    let mut fwd = Side::new();
    let mut bwd = Side::new();
    let mut freeze = FreezeChecker::new(board);
    let mut corral = CorralAnalyzer::new(board);

    // Shared scratch.
    let mut box_at = vec![false; board.num_squares];
    let mut box_list: Vec<u16> = Vec::new();
    let mut reach_stamp = vec![0u32; board.num_squares];
    let mut reach_gen = 0u32;
    let mut bfs_queue: Vec<u16> = Vec::with_capacity(board.num_squares);
    let mut frozen_flags: Vec<bool> = Vec::new();

    // Note: spliced solutions are not push-optimal in any mode (first meet
    // wins); OptimalPushes here just means both halves order by g + h.
    let f_of = |g: u32, h: u32| -> u64 {
        match opts.mode {
            Mode::OptimalPushes => g as u64 + h as u64,
            Mode::Weighted(w) => g as u64 + (w as u64) * (h as u64),
            Mode::Greedy => h as u64,
        }
    };
    let fwd_cost = |boxes: &[u16], i: usize, j: usize| -> Option<u32> {
        let d = board.goal_dist[j][boxes[i] as usize];
        (d != INF).then_some(d)
    };
    let bwd_cost = |boxes: &[u16], i: usize, j: usize| -> Option<u32> {
        let d = board.start_dist[j][boxes[i] as usize];
        (d != INF).then_some(d)
    };

    // Roots. Forward: the start state. Backward: the goal-filled board, one
    // root per player region (entries reuse box_from as the seed square).
    let root_h = match fwd.matcher.solve(board.start_boxes.len(), board.goals.len(), |i, j| {
        fwd_cost(&board.start_boxes, i, j)
    }) {
        Some(h) => h as u32,
        None => {
            stats.time = start_time.elapsed();
            return Outcome::Unsolvable { stats };
        }
    };
    fwd.open.push(Reverse((f_of(0, root_h), root_h, 0, NO_PARENT, 0, 0)));

    let goal_boxes: Box<[u16]> = {
        let mut g = board.goals.clone();
        g.sort_unstable();
        g.into_boxed_slice()
    };
    let bwd_root_h = match bwd.matcher.solve(goal_boxes.len(), board.start_boxes.len(), |i, j| {
        bwd_cost(&goal_boxes, i, j)
    }) {
        Some(h) => h as u32,
        None => {
            stats.time = start_time.elapsed();
            return Outcome::Unsolvable { stats };
        }
    };
    {
        for &b in goal_boxes.iter() {
            box_at[b as usize] = true;
        }
        reach_gen += 1;
        for sq in 0..board.num_squares as u16 {
            if box_at[sq as usize] || reach_stamp[sq as usize] == reach_gen {
                continue;
            }
            bfs_queue.clear();
            bfs_queue.push(sq);
            reach_stamp[sq as usize] = reach_gen;
            let mut head = 0;
            while head < bfs_queue.len() {
                let s = bfs_queue[head];
                head += 1;
                for d in 0..4 {
                    let n = board.neighbors[s as usize][d];
                    if n != NONE && reach_stamp[n as usize] != reach_gen && !box_at[n as usize] {
                        reach_stamp[n as usize] = reach_gen;
                        bfs_queue.push(n);
                    }
                }
            }
            bwd.seq += 1;
            let seq = bwd.seq;
            bwd.open.push(Reverse((
                f_of(0, bwd_root_h),
                bwd_root_h,
                u64::MAX - seq,
                NO_PARENT,
                sq,
                0,
            )));
        }
        for &b in goal_boxes.iter() {
            box_at[b as usize] = false;
        }
    }

    // Interleave: expand one node per side per round.
    let mut pops = 0u64;
    let mut forward_turn = true;
    loop {
        pops += 1;
        if stats.expanded >= opts.max_nodes
            || (pops % 512 == 0 && (start_time.elapsed() > opts.time_limit || opts.stopped()))
        {
            stats.time = start_time.elapsed();
            return Outcome::Exhausted { stats };
        }

        // Either side exhausting its complete search space without a meet is
        // already an unsolvability proof (forward covers everything reachable
        // from the start; backward everything that can reach the goal).
        if fwd.open.is_empty() || bwd.open.is_empty() {
            stats.time = start_time.elapsed();
            return Outcome::Unsolvable { stats };
        }
        let fwd_turn = forward_turn;
        forward_turn = !forward_turn;

        let side = if fwd_turn { &mut fwd } else { &mut bwd };
        let Some(Reverse((_, h, _, parent, box_from, dir))) = side.open.pop() else {
            continue;
        };

        // Materialize.
        let (boxes, player, g) = if parent == NO_PARENT {
            if fwd_turn {
                (board.start_boxes.clone().into_boxed_slice(), board.start_player, 0)
            } else {
                (goal_boxes.clone(), box_from, 0)
            }
        } else {
            let p = &side.arena[parent as usize];
            let to = board.neighbors[box_from as usize][dir as usize];
            let mut boxes = p.boxes.clone();
            let idx = boxes.iter().position(|&b| b == box_from).unwrap();
            boxes[idx] = to;
            boxes.sort_unstable();
            let player = if fwd_turn {
                box_from // pusher ends on the box's old square
            } else {
                board.neighbors[to as usize][dir as usize] // puller backs onto b+2d
            };
            (boxes, player, p.g + 1)
        };

        // Reachability + normalization.
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

        let key: Key = (boxes.clone(), norm);
        match side.tt.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if side.arena[*e.get() as usize].g <= g {
                    stats.duplicates += 1;
                    continue;
                }
                let node_idx = side.arena.len() as u32;
                e.insert(node_idx);
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(side.arena.len() as u32);
            }
        }
        let node_idx = side.arena.len() as u32;
        side.arena.push(BNode { boxes, g, parent, push_box_from: box_from, push_dir: dir });
        stats.expanded += 1;

        // Probe the other side: a key hit means same boxes AND same player
        // region — the two half paths compose.
        let other = if fwd_turn { &bwd } else { &fwd };
        if let Some(&other_idx) = other.tt.get(&key) {
            let (f_node, f_side, b_node, b_side) = if fwd_turn {
                (node_idx, &fwd, other_idx, &bwd)
            } else {
                (other_idx, &fwd, node_idx, &bwd)
            };
            let mut pushes = Vec::new();
            let mut cur = f_node;
            while f_side.arena[cur as usize].parent != NO_PARENT {
                let n = &f_side.arena[cur as usize];
                pushes.push((n.push_box_from, n.push_dir));
                cur = n.parent;
            }
            pushes.reverse();
            // Backward chain, walked from the met state toward the goal
            // state: each pull, reversed, is the forward push
            // (from = pull destination, dir = OPP[pull dir]).
            let mut cur = b_node;
            while b_side.arena[cur as usize].parent != NO_PARENT {
                let n = &b_side.arena[cur as usize];
                let pulled_to = board.neighbors[n.push_box_from as usize][n.push_dir as usize];
                pushes.push((pulled_to, OPP[n.push_dir as usize] as u8));
                cur = n.parent;
            }
            stats.time = start_time.elapsed();
            return Outcome::Solved { pushes, stats };
        }

        // Expand.
        if fwd_turn {
            // Forward: solved check then push generation with full pruning.
            let node = &fwd.arena[node_idx as usize];
            if h == 0 && node.boxes.iter().all(|&b| board.is_goal[b as usize]) {
                let mut pushes = Vec::new();
                let mut cur = node_idx;
                while fwd.arena[cur as usize].parent != NO_PARENT {
                    let n = &fwd.arena[cur as usize];
                    pushes.push((n.push_box_from, n.push_dir));
                    cur = n.parent;
                }
                pushes.reverse();
                stats.time = start_time.elapsed();
                return Outcome::Solved { pushes, stats };
            }

            let candidates = if opts.corral {
                match corral.analyze(
                    board,
                    &box_at,
                    |sq| reach_stamp[sq as usize] == reach_gen,
                    player,
                    &mut freeze,
                    true,
                    if parent == NO_PARENT { None } else { Some(player) }.map(|_| {
                        board.neighbors[box_from as usize][dir as usize]
                    }),
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

            let node_boxes = &fwd.arena[node_idx as usize].boxes;
            if crate::deadlock::scan_frozen(
                &mut freeze,
                board,
                &box_at,
                node_boxes,
                &mut frozen_flags,
            ) {
                stats.deadlocks += 1;
                continue;
            }
            let frozen = &frozen_flags;
            if fwd
                .matcher
                .solve(node_boxes.len(), board.goals.len(), |i, j| {
                    if frozen[i] {
                        return (board.goals[j] == node_boxes[i]).then_some(0);
                    }
                    fwd_cost(node_boxes, i, j)
                })
                .is_none()
            {
                stats.deadlocks += 1;
                continue;
            }
            fwd.matcher.snapshot();

            let try_push = |bi: usize, d: usize, open: &mut BinaryHeap<Entry>,
                                matcher: &mut Matcher, seq: &mut u64,
                                box_at: &mut Vec<bool>, freeze: &mut FreezeChecker,
                                stats: &mut Stats| {
                let b = node_boxes[bi];
                let to = board.neighbors[b as usize][d];
                if to == NONE || box_at[to as usize] || board.dead[to as usize] {
                    return;
                }
                let behind = board.neighbors[b as usize][OPP[d]];
                if behind == NONE
                    || box_at[behind as usize]
                    || reach_stamp[behind as usize] != reach_gen
                {
                    return;
                }
                box_at[b as usize] = false;
                box_at[to as usize] = true;
                let frozen_dead = freeze.is_freeze_deadlock(board, box_at, to);
                let h_child = if frozen_dead {
                    None
                } else {
                    let bi_locked =
                        board.is_goal[to as usize] && freeze.frozen(board, box_at, to);
                    matcher.restore();
                    matcher.resolve_row(bi, |i, j| {
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
                    })
                };
                box_at[to as usize] = false;
                box_at[b as usize] = true;
                let Some(h_child) = h_child else {
                    stats.deadlocks += 1;
                    return;
                };
                *seq += 1;
                stats.generated += 1;
                open.push(Reverse((
                    f_of(g + 1, h_child as u32),
                    h_child as u32,
                    u64::MAX - *seq,
                    node_idx,
                    b,
                    d as u8,
                )));
            };
            match candidates {
                Some(restricted) => {
                    for (b, d) in restricted {
                        let bi = node_boxes.iter().position(|&x| x == b).unwrap();
                        try_push(bi, d as usize, &mut fwd.open, &mut fwd.matcher,
                                 &mut fwd.seq, &mut box_at, &mut freeze, &mut stats);
                    }
                }
                None => {
                    for bi in 0..node_boxes.len() {
                        for d in 0..4 {
                            try_push(bi, d, &mut fwd.open, &mut fwd.matcher,
                                     &mut fwd.seq, &mut box_at, &mut freeze, &mut stats);
                        }
                    }
                }
            }
        } else {
            // Backward: solved when boxes sit on the start squares and the
            // player region contains the start position.
            let node_boxes = &bwd.arena[node_idx as usize].boxes;
            if h == 0
                && reach_stamp[board.start_player as usize] == reach_gen
                && node_boxes.as_ref() == board.start_boxes.as_slice()
            {
                let mut pushes = Vec::new();
                let mut cur = node_idx;
                while bwd.arena[cur as usize].parent != NO_PARENT {
                    let n = &bwd.arena[cur as usize];
                    let pulled_to = board.neighbors[n.push_box_from as usize][n.push_dir as usize];
                    pushes.push((pulled_to, OPP[n.push_dir as usize] as u8));
                    cur = n.parent;
                }
                stats.time = start_time.elapsed();
                return Outcome::Solved { pushes, stats };
            }

            if bwd
                .matcher
                .solve(node_boxes.len(), board.start_boxes.len(), |i, j| {
                    bwd_cost(node_boxes, i, j)
                })
                .is_none()
            {
                continue;
            }
            bwd.matcher.snapshot();

            for bi in 0..node_boxes.len() {
                let b = node_boxes[bi];
                for d in 0..4 {
                    let to = board.neighbors[b as usize][d];
                    if to == NONE
                        || box_at[to as usize]
                        || reach_stamp[to as usize] != reach_gen
                        || board.backward_dead[to as usize]
                    {
                        continue;
                    }
                    let beyond = board.neighbors[to as usize][d];
                    if beyond == NONE || box_at[beyond as usize] {
                        continue;
                    }
                    bwd.matcher.restore();
                    let h_child = bwd.matcher.resolve_row(bi, |i, j| {
                        let sq = if i == bi { to } else { node_boxes[i] };
                        let dist = board.start_dist[j][sq as usize];
                        (dist != INF).then_some(dist)
                    });
                    let Some(h_child) = h_child else {
                        stats.deadlocks += 1;
                        continue;
                    };
                    bwd.seq += 1;
                    let seq = bwd.seq;
                    stats.generated += 1;
                    bwd.open.push(Reverse((
                        f_of(g + 1, h_child as u32),
                        h_child as u32,
                        u64::MAX - seq,
                        node_idx,
                        b,
                        d as u8,
                    )));
                }
            }
        }
    }
}
