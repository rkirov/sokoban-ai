//! Push-based best-first search.
//!
//! States are (box multiset, player-reachability component), the component
//! canonicalized as the minimum-index square the player can reach. The search
//! runs over pushes; player walking between pushes is reconstructed later.
//!
//! Heap entries are lazy: a child is described by (parent arena index, push)
//! and only materialized (reachability BFS, transposition check, arena node)
//! when popped. With a consistent heuristic (min-cost matching over relaxed
//! push distances is 1-Lipschitz per push) the first pop of a state has
//! minimal g, so pop-time dedup preserves optimality in OptimalPushes mode.

use crate::corral::{CorralAnalyzer, CorralResult};
use crate::deadlock::FreezeChecker;
use crate::level::{Board, NONE};
use crate::matching::{min_cost_matching, Matcher};
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A*: f = g + h. Solutions are push-optimal.
    OptimalPushes,
    /// Weighted A*: f = g + w * h. Faster, not optimal.
    Weighted(u32),
    /// Greedy best-first: f = h.
    Greedy,
}

#[derive(Clone)]
pub struct Options {
    pub mode: Mode,
    pub max_nodes: u64,
    pub time_limit: Duration,
    pub corral: bool,
    /// Cooperative cancellation for racing strategies: when set, the search
    /// returns Exhausted at its next periodic check.
    pub stop: Arc<AtomicBool>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::OptimalPushes,
            max_nodes: u64::MAX,
            time_limit: Duration::from_secs(60),
            corral: true,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Options {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

#[derive(Default, Clone)]
pub struct Stats {
    pub expanded: u64,
    pub generated: u64,
    pub deadlocks: u64,
    pub duplicates: u64,
    pub time: Duration,
}

pub enum Outcome {
    /// Push sequence as (box_from_square, direction).
    Solved { pushes: Vec<(u16, u8)>, stats: Stats },
    Unsolvable { stats: Stats },
    /// Node or time limit hit.
    Exhausted { stats: Stats },
}

const OPP: [usize; 4] = [1, 0, 3, 2];

struct Node {
    boxes: Box<[u16]>,
    /// Actual (not normalized) player square; kept for debugging (the search
    /// re-derives it during materialization, reconstruction re-walks it).
    #[allow(dead_code)]
    player: u16,
    g: u32,
    parent: u32,
    push_box_from: u16,
    push_dir: u8,
}

// Naive tunnel macros (force a box through any wall-flanked goal-free run)
// are UNSOUND: a box may need to park inside a tunnel to vacate a square for
// the player (Microban I #10, tests::tunnel_parking_is_required). The sound
// replacement is the gate-push rule, level.rs `forced_pushes`.

const NO_PARENT: u32 = u32::MAX;

enum Candidates {
    All,
    Restricted(Vec<(u16, u8)>),
}

/// Open list for small integer keys: pops the lowest f, then the lowest h,
/// newest first (LIFO keeps the search focused) — the order a binary heap
/// over (f, h, newest-first) gives, in O(1) per operation instead of
/// O(log n), with 8-byte entries. An entry is (parent, box_from, dir); the
/// child state is materialized only when popped.
struct BucketQueue {
    /// buckets[f][h]: entries with that key, pushed in order.
    buckets: Vec<Vec<Vec<(u32, u16, u8)>>>,
    /// min_h[f]: no entry with this f has a smaller h.
    min_h: Vec<usize>,
    /// No entry has a smaller f (pushes may lower it: weighted f can drop).
    min_f: usize,
    len: usize,
}

impl BucketQueue {
    fn new() -> Self {
        BucketQueue { buckets: Vec::new(), min_h: Vec::new(), min_f: usize::MAX, len: 0 }
    }

    fn push(&mut self, f: u64, h: u32, entry: (u32, u16, u8)) {
        let (f, h) = (f as usize, h as usize);
        if self.buckets.len() <= f {
            self.buckets.resize_with(f + 1, Vec::new);
            self.min_h.resize(f + 1, usize::MAX);
        }
        let row = &mut self.buckets[f];
        if row.len() <= h {
            row.resize_with(h + 1, Vec::new);
        }
        row[h].push(entry);
        self.min_h[f] = self.min_h[f].min(h);
        self.min_f = self.min_f.min(f);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(u32, (u32, u16, u8))> {
        if self.len == 0 {
            return None;
        }
        loop {
            let f = self.min_f;
            let row = &mut self.buckets[f];
            let start = self.min_h[f].min(row.len());
            if let Some(offset) = row[start..].iter().position(|b| !b.is_empty()) {
                let h = start + offset;
                self.min_h[f] = h;
                self.len -= 1;
                return Some((h as u32, row[h].pop().unwrap()));
            }
            self.min_h[f] = usize::MAX;
            self.min_f += 1;
        }
    }
}

pub fn solve(board: &Board, opts: &Options) -> Outcome {
    let start_time = Instant::now();
    let mut stats = Stats::default();

    let mut arena: Vec<Node> = Vec::new();
    let mut tt: FxHashMap<(Box<[u16]>, u16), u32> = FxHashMap::default();
    let mut open = BucketQueue::new();
    let mut freeze = FreezeChecker::new(board);
    let dead_sets = crate::deadsets::DeadSetTables::new(board, crate::deadsets::Direction::Forward);
    let mut corral = CorralAnalyzer::new(board);
    let mut matcher = Matcher::new();
    let equal_goals_boxes = board.goals.len() == board.start_boxes.len();

    // Scratch buffers reused across pops.
    let mut box_at = vec![false; board.num_squares];
    let mut box_list: Vec<u16> = Vec::new();
    let mut reach_stamp = vec![0u32; board.num_squares];
    let mut reach_gen = 0u32;
    let mut bfs_queue: Vec<u16> = Vec::with_capacity(board.num_squares);
    let mut frozen_flags: Vec<bool> = Vec::new();
    let mut frozen_squares: Vec<u16> = Vec::new();
    let mut frozen_walls = crate::deadlock::FrozenWalls::new(board);

    let f_of = |g: u32, h: u32| -> u64 {
        match opts.mode {
            Mode::OptimalPushes => g as u64 + h as u64,
            Mode::Weighted(w) => g as u64 + (w as u64) * (h as u64),
            Mode::Greedy => h as u64,
        }
    };

    let heuristic = |boxes: &[u16]| -> Option<u64> {
        min_cost_matching(boxes.len(), board.goals.len(), |i, j| {
            let d = board.goal_dist[j][boxes[i] as usize];
            (d != crate::level::INF).then_some(d)
        })
    };

    let root_h = match heuristic(&board.start_boxes) {
        Some(h) => h,
        None => return Outcome::Unsolvable { stats },
    };
    open.push(f_of(0, root_h as u32), root_h as u32, (NO_PARENT, 0, 0));

    let mut pops = 0u64;
    while let Some((h, (parent, box_from, dir))) = open.pop() {
        pops += 1;
        if stats.expanded >= opts.max_nodes
            || (pops % 512 == 0 && (start_time.elapsed() > opts.time_limit || opts.stopped()))
        {
            stats.time = start_time.elapsed();
            return Outcome::Exhausted { stats };
        }

        // Materialize the child state.
        let (boxes, player, g, focus) = if parent == NO_PARENT {
            (board.start_boxes.clone().into_boxed_slice(), board.start_player, 0, None)
        } else {
            let p = &arena[parent as usize];
            let to = board.neighbors[box_from as usize][dir as usize];
            let mut boxes = p.boxes.clone();
            let idx = boxes.iter().position(|&b| b == box_from).unwrap();
            boxes[idx] = to;
            boxes.sort_unstable();
            (boxes, box_from, p.g + 1, Some(to))
        };

        // Player reachability BFS; the minimum reachable square canonicalizes
        // the player component for the transposition table.
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

        match tt.entry(board.canonical_key(&boxes, &bfs_queue, norm)) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if *e.get() <= g {
                    stats.duplicates += 1;
                    continue;
                }
                e.insert(g);
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(g);
            }
        }

        let node_idx = arena.len() as u32;
        arena.push(Node { boxes, player, g, parent, push_box_from: box_from, push_dir: dir });
        stats.expanded += 1;

        if h == 0 {
            let node = &arena[node_idx as usize];
            if node.boxes.iter().all(|&b| board.is_goal[b as usize]) {
                stats.time = start_time.elapsed();
                let mut pushes = Vec::with_capacity(g as usize);
                let mut cur = node_idx;
                while arena[cur as usize].parent != NO_PARENT {
                    let n = &arena[cur as usize];
                    pushes.push((n.push_box_from, n.push_dir));
                    cur = n.parent;
                }
                pushes.reverse();
                return Outcome::Solved { pushes, stats };
            }
        }

        // Gate push (see level.rs `forced_pushes`): the box just pushed can
        // only usefully continue forward, so that is the only move here.
        // Otherwise PI-corral analysis: when a qualifying corral exists,
        // only its fence pushes need to be considered at this node.
        let candidates: Candidates = if parent != NO_PARENT && board.forced[box_from as usize][dir as usize] {
            let to = board.neighbors[box_from as usize][dir as usize];
            Candidates::Restricted(vec![(to, dir)])
        } else if opts.corral {
            match corral.analyze(
                board,
                &box_at,
                |sq| reach_stamp[sq as usize] == reach_gen,
                player,
                &mut freeze,
                equal_goals_boxes,
                focus,
            ) {
                CorralResult::Deadlock => {
                    stats.deadlocks += 1;
                    continue;
                }
                CorralResult::Restrict(pushes) => Candidates::Restricted(pushes),
                CorralResult::NoPruning => Candidates::All,
            }
        } else {
            Candidates::All
        };

        // Node-level matching state: full solve once, then each child's h by
        // re-augmenting only the moved box's row from the snapshot. O(n m)
        // per child instead of O(n^3). Boxes frozen on goals get their row
        // locked to that goal (they can never move again); a frozen box off
        // a goal kills the node outright.
        let node = &arena[node_idx as usize];
        let node_boxes = &node.boxes;
        if crate::deadlock::scan_frozen(&mut freeze, board, &box_at, node_boxes, &mut frozen_flags)
        {
            stats.deadlocks += 1;
            continue;
        }
        let frozen = &frozen_flags;
        // Distances with boxes frozen on goals as walls (they never move
        // again, so the walls hold in every descendant: still a lower bound;
        // see deadlock::FrozenWalls).
        frozen_squares.clear();
        frozen_squares.extend(node_boxes.iter().zip(frozen.iter()).filter(|p| *p.1).map(|p| *p.0));
        let walled = (!frozen_squares.is_empty()).then(|| frozen_walls.distances(board, &frozen_squares));
        let dist_table = walled.as_deref().unwrap_or(&board.goal_dist);
        let node_cost = |i: usize, j: usize| -> Option<u32> {
            if frozen[i] {
                return (board.goals[j] == node_boxes[i]).then_some(0);
            }
            let d = dist_table[j][node_boxes[i] as usize];
            (d != crate::level::INF).then_some(d)
        };
        if matcher.solve(node_boxes.len(), board.goals.len(), node_cost).is_none() {
            stats.deadlocks += 1;
            continue; // frozen boxes claim goals others need: dead position
        }
        matcher.snapshot();

        // Expand: try each candidate push (box index, direction) with the
        // player side reachable. Restricted pushes get the same checks.
        let try_push = |bi: usize,
                            d: usize,
                            box_at: &mut Vec<bool>,
                            freeze: &mut FreezeChecker,
                            matcher: &mut Matcher,
                            stats: &mut Stats,
                            open: &mut BucketQueue| {
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

            // Freeze-deadlock check on the position after the push. If the
            // pushed box freezes ON a goal its row locks to that goal.
            box_at[b as usize] = false;
            box_at[to as usize] = true;
            let frozen_dead = freeze.is_freeze_deadlock(board, box_at, to)
                || dead_sets.moved_box_dead(node_boxes, bi, to, b);
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
                    let dist = dist_table[j][sq as usize];
                    (dist != crate::level::INF).then_some(dist)
                })
            };
            box_at[to as usize] = false;
            box_at[b as usize] = true;

            let Some(h_child) = h_child else {
                stats.deadlocks += 1;
                return;
            };
            stats.generated += 1;
            open.push(f_of(node.g + 1, h_child as u32), h_child as u32, (node_idx, b, d as u8));
        };
        match candidates {
            Candidates::All => {
                for bi in 0..node_boxes.len() {
                    for d in 0..4 {
                        try_push(bi, d, &mut box_at, &mut freeze, &mut matcher, &mut stats, &mut open);
                    }
                }
            }
            Candidates::Restricted(pushes) => {
                for (b, d) in pushes {
                    let bi = node_boxes.iter().position(|&x| x == b).unwrap();
                    try_push(bi, d as usize, &mut box_at, &mut freeze, &mut matcher, &mut stats, &mut open);
                }
            }
        }
    }

    stats.time = start_time.elapsed();
    Outcome::Unsolvable { stats }
}

/// Backward (pull) search: start from the goal-filled board and pull boxes
/// until they sit on the level's start squares with the player connected to
/// the start position. Levels that are cramped around their goals are often
/// far easier in this direction. Returns *forward* pushes on success.
///
/// A pull of box `b` in direction d: player walks to `b+d`, steps to `b+2d`,
/// the box follows onto `b+d`. Reversed, that is the forward push
/// (from `b+d`, direction OPP[d]).
///
/// Only valid when #boxes == #goals (the goal-filled start would otherwise be
/// ambiguous); the caller checks. No freeze/corral analogue is applied —
/// backward-dead squares and the matching bound do the pruning.
pub fn solve_backward(board: &Board, opts: &Options) -> Outcome {
    let start_time = Instant::now();
    let mut stats = Stats::default();

    if board.goals.len() != board.start_boxes.len() {
        stats.time = start_time.elapsed();
        return Outcome::Exhausted { stats };
    }

    let mut arena: Vec<Node> = Vec::new();
    let mut tt: FxHashMap<(Box<[u16]>, u16), u32> = FxHashMap::default();
    let mut open = BucketQueue::new();
    let mut matcher = Matcher::new();

    let mut box_at = vec![false; board.num_squares];
    let mut box_list: Vec<u16> = Vec::new();
    let mut reach_stamp = vec![0u32; board.num_squares];
    let mut reach_gen = 0u32;
    let mut bfs_queue: Vec<u16> = Vec::with_capacity(board.num_squares);

    let f_of = |g: u32, h: u32| -> u64 {
        match opts.mode {
            Mode::OptimalPushes => g as u64 + h as u64,
            Mode::Weighted(w) => g as u64 + (w as u64) * (h as u64),
            Mode::Greedy => h as u64,
        }
    };

    let back_sets = crate::deadsets::DeadSetTables::new(board, crate::deadsets::Direction::Backward);
    let goal_boxes: Box<[u16]> = {
        let mut g = board.goals.clone();
        g.sort_unstable();
        g.into_boxed_slice()
    };
    let root_h = match min_cost_matching(goal_boxes.len(), board.start_boxes.len(), |i, j| {
        let d = board.start_dist[j][goal_boxes[i] as usize];
        (d != crate::level::INF).then_some(d)
    }) {
        Some(h) => h as u32,
        None => {
            stats.time = start_time.elapsed();
            return Outcome::Unsolvable { stats };
        }
    };

    // One root per player region of the goal-filled board (the forward
    // solution's final player position is in one of them). Root entries
    // reuse the box_from field to carry the region's seed player square.
    {
        for &b in goal_boxes.iter() {
            box_at[b as usize] = true;
        }
        reach_gen += 1;
        for sq in 0..board.num_squares as u16 {
            if box_at[sq as usize] || reach_stamp[sq as usize] == reach_gen {
                continue;
            }
            // Flood this region so each region seeds exactly one root.
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
            // Root entries carry the region's seed player square in box_from.
            open.push(f_of(0, root_h), root_h, (NO_PARENT, sq, 0));
        }
        for &b in goal_boxes.iter() {
            box_at[b as usize] = false;
        }
    }

    // Solved matching state per arena node (see Matcher::save).
    let mut states: Vec<i64> = Vec::new();
    let mut pops = 0u64;
    while let Some((h, (parent, box_from, dir))) = open.pop() {
        pops += 1;
        if stats.expanded >= opts.max_nodes
            || (pops % 512 == 0 && (start_time.elapsed() > opts.time_limit || opts.stopped()))
        {
            stats.time = start_time.elapsed();
            return Outcome::Exhausted { stats };
        }

        // Materialize: for a pull entry, box_from moves to b+d and the player
        // ends at b+2d.
        // Boxes stay in the parent's order (the matching's rows are box
        // indices, so a child can warm-start from its parent's matching with
        // only the moved row changed); keys and the goal test use a sorted
        // copy.
        let (boxes, player, g, moved) = if parent == NO_PARENT {
            (goal_boxes.clone(), box_from, 0, None)
        } else {
            let p = &arena[parent as usize];
            let to = board.neighbors[box_from as usize][dir as usize];
            let player = board.neighbors[to as usize][dir as usize];
            let mut boxes = p.boxes.clone();
            let idx = boxes.iter().position(|&b| b == box_from).unwrap();
            boxes[idx] = to;
            (boxes, player, p.g + 1, Some(idx))
        };
        let mut sorted = boxes.clone();
        sorted.sort_unstable();

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

        // Raw key, NOT `canonical_key`: board automorphisms preserve walls
        // and goals but not the start squares this search is aiming for, so
        // merging a state with its mirror image loses the paths that reach
        // the actual start (a mirror-symmetric one-push level was reported
        // unsolvable; see tests::backward_search_on_symmetric_board).
        match tt.entry((sorted.clone(), norm)) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if *e.get() <= g {
                    stats.duplicates += 1;
                    continue;
                }
                e.insert(g);
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(g);
            }
        }

        let node_idx = arena.len() as u32;
        arena.push(Node {
            boxes,
            player,
            g,
            parent,
            push_box_from: box_from,
            push_dir: dir,
        });
        stats.expanded += 1;

        // Solved: boxes on the start squares AND the player's region contains
        // the forward start position (h == 0 with exact distances implies the
        // box set equals the start set).
        if h == 0 && reach_stamp[board.start_player as usize] == reach_gen {
            if sorted.as_ref() == board.start_boxes.as_slice() {
                stats.time = start_time.elapsed();
                // Reversed pulls become forward pushes.
                let mut pushes = Vec::with_capacity(g as usize);
                let mut cur = node_idx;
                while arena[cur as usize].parent != NO_PARENT {
                    let n = &arena[cur as usize];
                    let pulled_to = board.neighbors[n.push_box_from as usize][n.push_dir as usize];
                    pushes.push((pulled_to, OPP[n.push_dir as usize] as u8));
                    cur = n.parent;
                }
                // Walking from the root: pulls were recorded backward-in-time,
                // and reversing the (already reverse-ordered) list yields...
                // the same backward order; the collection above walks from the
                // last pull to the first, which IS forward order for pushes.
                stats.time = start_time.elapsed();
                return Outcome::Solved { pushes, stats };
            }
        }

        // Node-level matching state against start squares.
        let node = &arena[node_idx as usize];
        let node_boxes = &node.boxes;
        let node_cost = |i: usize, j: usize| -> Option<u32> {
            let d = board.start_dist[j][node_boxes[i] as usize];
            (d != crate::level::INF).then_some(d)
        };
        let m = board.start_boxes.len();
        let solved = match moved {
            Some(row) => {
                let stride = 3 * (m + 1);
                let at = parent as usize * stride;
                matcher.load(node_boxes.len(), m, &states[at..at + stride]);
                matcher.resolve_row(row, node_cost)
            }
            None => matcher.solve(node_boxes.len(), m, node_cost),
        };
        // Every arena node gets a state slot (dead nodes keep a stale one;
        // they are never expanded further).
        matcher.save(&mut states);
        if solved.is_none() {
            continue;
        }
        matcher.snapshot();

        // Pulls: box at b, player reaches b+d, steps to b+2d.
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
                // Small box-set deadlocks for pull searches (see deadsets.rs).
                if back_sets.moved_box_dead(node_boxes, bi, to, beyond) {
                    continue;
                }

                matcher.restore();
                let h_child = matcher.resolve_row(bi, |i, j| {
                    let sq = if i == bi { to } else { node_boxes[i] };
                    let dist = board.start_dist[j][sq as usize];
                    (dist != crate::level::INF).then_some(dist)
                });
                let Some(h_child) = h_child else {
                    stats.deadlocks += 1;
                    continue;
                };
                stats.generated += 1;
                open.push(f_of(node.g + 1, h_child as u32), h_child as u32, (node_idx, b, d as u8));
            }
        }
    }

    stats.time = start_time.elapsed();
    Outcome::Unsolvable { stats }
}
