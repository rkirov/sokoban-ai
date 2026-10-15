//! Packing-order search: a forward search that walks YASS's packing order
//! (yassorder.rs), re-derived from YASS 2.153 and diff-tested against an
//! instrumented YASS build, expansion by expansion.
//!
//! Why it walks almost straight to a solution: each child is scored by the
//! pushed box's own distance to the nearest open target of the current
//! phase (weight 2), with a bonus for every target the push approaches, and
//! an approaching push is free for the children (its cost is refunded one
//! push later). Marching one box toward its target therefore lowers the
//! score by at least one per push, and a child that is no worse than its
//! parent and the best open entry is expanded at once, depth first, inside
//! the generation loop. Every filled target is worth 32, so once a phase is
//! done the older open entries never compete again; a box leaving a target
//! of an earlier phase sets the phase back and loses that credit.
//!
//! The search is greedy and incomplete; in the portfolio it gets a short
//! time slice (main.rs).

use crate::deadlock::FreezeChecker;
use crate::deadsets::{DeadSetTables, Direction};
use crate::level::{Board, INF, NONE, OPP};
use crate::solver::{Options, Outcome, Stats};
use crate::yasscorral::YassCorral;
use crate::yassorder::OrderEntry;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::time::Instant;

const PER_TARGET: i64 = 32;
/// Scores start this high so that bonuses can be subtracted.
const OFFSET: i64 = 255 * PER_TARGET;
const NO_PARENT: u32 = u32::MAX;
const MAX_DIVE: usize = 4000;
/// Learned dead corrals: at most this many boxes, proved within this many
/// positions.
const LEARN_BOXES: usize = 5;
const PROOF_STATES: usize = 500;

/// One target of the order: a square filled in a phase. A square can be a
/// target of two phases (a goal used for parking and filled again later).
#[derive(Clone, Copy)]
struct Entry {
    sq: u16,
    phase: u16,
    /// A parking target: its box moves on to its goal in phase `home`.
    parking: bool,
    home: u16,
}

/// The order as phases: they count down, the highest is filled first, and
/// phase 0 means done.
struct Phases {
    entries: Vec<Entry>,
    /// members[p]: the entries of phase p (index 0 unused).
    members: Vec<Vec<usize>>,
    /// at[sq]: the entries at a square, ascending by phase.
    at: Vec<Vec<usize>>,
    /// before[p]: number of entries in the phases above p (already done).
    before: Vec<i64>,
    /// dist[entry][sq]: player-aware push distance from sq to the entry's
    /// square, with the real goals of earlier phases as walls.
    dist: Vec<Vec<u32>>,
    count: u16,
}

impl Phases {
    fn new(board: &Board, order: &[OrderEntry]) -> Self {
        let entries: Vec<Entry> = order.iter().map(|&(sq, phase, parking, home)| Entry { sq, phase, parking, home }).collect();
        let n = board.num_squares;
        let count = entries.iter().map(|e| e.phase).max().unwrap_or(0);
        let mut members = vec![Vec::new(); count as usize + 1];
        let mut at = vec![Vec::new(); n];
        for (k, e) in entries.iter().enumerate() {
            members[e.phase as usize].push(k);
            at[e.sq as usize].push(k);
        }
        for a in at.iter_mut() {
            a.sort_by_key(|&k| entries[k].phase);
        }
        let mut before = vec![0i64; count as usize + 1];
        for p in (1..count as usize).rev() {
            before[p] = before[p + 1] + members[p + 1].len() as i64;
        }
        let mut tables: FxHashMap<(u16, u16), Vec<u32>> = FxHashMap::default();
        let mut dist = Vec::with_capacity(entries.len());
        for e in &entries {
            let table = tables.entry((e.sq, e.phase)).or_insert_with(|| {
                let wall = |s: usize| at[s].iter().any(|&k| !entries[k].parking && entries[k].phase > e.phase);
                let nb: Vec<[u16; 4]> = (0..n)
                    .map(|q| if wall(q) { [NONE; 4] } else { board.neighbors[q].map(|x| if x != NONE && wall(x as usize) { NONE } else { x }) })
                    .collect();
                aware_distances(&nb, e.sq)
            });
            dist.push(table.clone());
        }
        Phases { entries, members, at, before, dist, count }
    }

    fn is_filled(&self, p: u16, box_at: &[bool]) -> bool {
        self.members[p as usize].iter().all(|&k| box_at[self.entries[k].sq as usize])
    }

    /// Lower the phase while its targets are all filled.
    fn settle(&self, mut phase: u16, box_at: &[bool]) -> u16 {
        while phase > 0 && self.is_filled(phase, box_at) {
            phase -= 1;
        }
        phase
    }

    /// The phase after a box leaves `from`. Leaving a target of an earlier
    /// phase sets the phase back to it, unless it is a parked box whose time
    /// has come (or, in the last two phases, any parked box). Leaving a
    /// current target that an earlier phase also fills sets it back too.
    fn after_leaving(&self, phase: u16, from: u16) -> u16 {
        let es = &self.at[from as usize];
        let Some(&top) = es.last() else { return phase };
        if self.entries[top].phase < phase {
            return phase;
        }
        let i = es.iter().position(|&k| self.entries[k].phase >= phase).unwrap();
        let e = self.entries[es[i]];
        if e.phase > phase {
            let home = if e.parking { e.home } else { 0 };
            if home < phase && (!e.parking || phase > 2) {
                return e.phase;
            }
            phase
        } else if phase < self.count && i + 1 < es.len() {
            self.entries[es[i + 1]].phase
        } else {
            phase
        }
    }

    /// Score of a child reached by pushing a box from `from` to `to`
    /// (`box_at` is the child's occupancy; `base` its pushes, refunds and
    /// lower bound). Returns (phase, score, approaching).
    fn score(&self, parent_phase: u16, base: i64, from: u16, to: u16, box_at: &[bool]) -> (u16, i64, bool) {
        let phase = if parent_phase > 0 { self.after_leaving(parent_phase, from) } else { 0 };
        if phase == 0 {
            return (0, base, false);
        }
        let (mut approach, mut landed, mut filled, mut dmin) = (0i64, 0i64, 0i64, INF);
        for &k in &self.members[phase as usize] {
            let t = self.entries[k].sq;
            if box_at[t as usize] {
                filled += 1;
                if t == to {
                    landed = 1;
                    dmin = 0;
                }
            } else {
                let d = &self.dist[k];
                dmin = dmin.min(d[to as usize]);
                if d[to as usize] < d[from as usize] {
                    approach += 2;
                }
            }
        }
        let tagged = approach + landed > 0;
        if filled as usize == self.members[phase as usize].len() {
            // Phase complete: credit the phases now done.
            let p = self.settle(phase, box_at);
            let f = if p > 0 { self.members[p as usize].iter().filter(|&&k| box_at[self.entries[k].sq as usize]).count() as i64 } else { 0 };
            let bonus = PER_TARGET * (self.before.get(p as usize).copied().unwrap_or(0) + f) - 8;
            return (p, (base - bonus).max(1), tagged);
        }
        let mut bonus = approach + landed + PER_TARGET * (self.before[phase as usize] + filled);
        bonus -= if dmin != INF { 2 * (1 + dmin as i64) } else { 8 };
        (phase, (base - bonus).max(1), tagged)
    }

    /// Is a box frozen on a real goal that a phase below `phase` fills?
    fn frozen_early(&self, phase: u16, board: &Board, box_at: &[bool], freeze: &mut FreezeChecker) -> i64 {
        let mut n = 0;
        for p in 1..phase {
            for &k in &self.members[p as usize] {
                let e = self.entries[k];
                if !e.parking && box_at[e.sq as usize] && freeze.frozen(board, box_at, e.sq) {
                    n += 1;
                }
            }
        }
        n
    }
}

struct Node {
    /// Boxes keep their index along a path (generation order depends on it).
    boxes: Box<[u16]>,
    player: u16,
    g: u32,
    /// Approaching pushes on the path, this node's own included.
    tagged: u32,
    phase: u16,
    score: i64,
    /// Some box may be frozen on a later phase's goal.
    frozen_early: bool,
    expanded: bool,
    /// On the open list (entries of rescored nodes go stale).
    queued: bool,
    parent: u32,
    from: u16,
    dir: u8,
}

struct Search<'a> {
    board: &'a Board,
    ph: Phases,
    nodes: Vec<Node>,
    /// (sorted boxes, player region's smallest square) -> node.
    tt: FxHashMap<(Box<[u16]>, u16), u32>,
    /// Open list: buckets by score, last in first out.
    buckets: Vec<Vec<u32>>,
    open_min: usize,
    /// live[s]: queued nodes with score s (buckets may hold stale entries).
    live: Vec<u32>,
    rover: usize,
    /// Pushes generated so far; the rover fires on multiples of 8.
    generated: u64,
    freeze: FreezeChecker,
    dead_sets: DeadSetTables,
    corral: YassCorral,
    /// Dead corrals learned so far, as (boxes, the player's region with
    /// only those boxes on the board, a square inside the corral), indexed
    /// by box square; and the proofs tried, by (boxes, player region).
    patterns: Vec<(Box<[u16]>, u16, u16)>,
    patterns_at: Vec<Vec<u32>>,
    proved: FxHashMap<(Box<[u16]>, u16), bool>,
    start: Instant,
    opts: &'a Options,
    solved: Option<u32>,
}

impl<'a> Search<'a> {
    fn push_open(&mut self, idx: u32, score: i64) {
        let s = score as usize;
        if self.buckets.len() <= s {
            self.buckets.resize_with(s + 1, Vec::new);
            self.live.resize(s + 1, 0);
        }
        self.buckets[s].push(idx);
        self.live[s] += 1;
        self.nodes[idx as usize].queued = true;
        self.open_min = self.open_min.min(s);
    }

    /// Take a queued node off the open list (its entry goes stale).
    fn unqueue(&mut self, idx: u32) {
        let n = &mut self.nodes[idx as usize];
        if n.queued {
            n.queued = false;
            self.live[n.score as usize] -= 1;
        }
    }

    /// The best score on the open list (usize::MAX if empty).
    fn open_min(&mut self) -> usize {
        while self.open_min < self.buckets.len() && self.live[self.open_min] == 0 {
            self.buckets[self.open_min].clear();
            self.open_min += 1;
        }
        if self.open_min >= self.buckets.len() {
            self.open_min = usize::MAX;
        }
        self.open_min
    }

    fn pop_open(&mut self) -> Option<u32> {
        let mut b = self.open_min();
        if b == usize::MAX {
            return None;
        }
        // Now and then take the next non-empty bucket above the last such
        // pick instead of the best (escapes a futile line), wrapping around.
        if self.generated % 8 == 0 {
            let mut r = self.rover + 1;
            while r < self.buckets.len() && self.live[r] == 0 {
                r += 1;
            }
            self.rover = if r < self.buckets.len() { r } else { b };
            b = self.rover;
        }
        // Last in, first out; skip stale entries.
        while let Some(idx) = self.buckets[b].pop() {
            let n = &self.nodes[idx as usize];
            if n.queued && n.score as usize == b {
                self.unqueue(idx);
                return Some(idx);
            }
        }
        unreachable!("a bucket with live entries ran empty")
    }

    fn out_of_time(&self) -> bool {
        self.start.elapsed() > self.opts.time_limit || self.opts.stopped() || self.nodes.len() as u64 >= self.opts.max_nodes
    }

    /// Expand node `idx`: generate its pushes; dive into a child that is no
    /// worse than this node and the best open entry, queue the others.
    fn expand(&mut self, idx: u32, depth: usize) {
        if self.solved.is_some() || (self.nodes.len() % 1024 == 0 && self.out_of_time()) {
            return;
        }
        let board = self.board;
        self.nodes[idx as usize].expanded = true;
        let n = &self.nodes[idx as usize];
        let (boxes, player, g, tagged, phase, score, frozen_early) =
            (n.boxes.clone(), n.player, n.g, n.tagged, n.phase, n.score, n.frozen_early);
        let last = (n.parent != NO_PARENT).then(|| board.neighbors[n.from as usize][n.dir as usize]);
        let mut box_at = vec![false; board.num_squares];
        for &b in boxes.iter() {
            box_at[b as usize] = true;
        }
        let (reach, _) = flood(board, &box_at, player);

        if self.learn_dead_corral(&box_at, &reach, player) {
            return;
        }
        // After a gate or tunnel push only the pushed box moves next, and
        // corrals are not examined. Otherwise corral pruning: only the
        // corral's pushes. A push that works on a corral keeps this node's
        // score, so opening the corral is tried before it is rejected.
        let n = &self.nodes[idx as usize];
        let gate = n.parent != NO_PARENT && (board.forced[n.from as usize][n.dir as usize] || tunnel(board, n.from, n.dir as usize));
        let corral = if gate { None } else { self.corral.analyze(board, &box_at, &reach, &mut self.freeze, &boxes) };
        let inherit = idx != 0 && corral.is_some();

        // The last pushed box first, then the others ascending after an odd
        // number of pushes and descending after an even one; directions up,
        // left, down, right. Equal children are tried in this order.
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        if g % 2 == 0 {
            order.reverse();
        }
        if let Some(p) = last.and_then(|l| boxes.iter().position(|&b| b == l)) {
            order.retain(|&x| x != p);
            order.insert(0, p);
        }
        let slb0: u32 = boxes.iter().map(|&b| board.min_goal_dist[b as usize]).sum();
        for bi in order {
            let b = boxes[bi];
            if gate && Some(b) != last {
                continue;
            }
            for d in [0usize, 2, 1, 3] {
                if corral.as_ref().is_some_and(|c| !c.contains(&(b, d as u8))) {
                    continue;
                }
                let to = board.neighbors[b as usize][d];
                let behind = board.neighbors[b as usize][OPP[d]];
                if to == NONE || behind == NONE || box_at[to as usize] || board.dead[to as usize] || !reach[behind as usize] {
                    continue;
                }
                box_at[b as usize] = false;
                box_at[to as usize] = true;
                if !self.freeze.is_freeze_deadlock(board, &box_at, to) && !self.dead_sets.moved_box_dead(&boxes, bi, to, b) {
                    self.generated += 1;
                    let mut child = boxes.to_vec();
                    child[bi] = to;
                    self.add_child(idx, child, bi, d, &mut box_at, depth, (g, tagged, phase, score, frozen_early, slb0), inherit);
                }
                box_at[to as usize] = false;
                box_at[b as usize] = true;
                if self.solved.is_some() {
                    return;
                }
            }
        }
    }

    /// Score and store the child that pushed box `bi` in direction `d`
    /// (`box_at` is the child's occupancy), then dive into it or queue it.
    #[allow(clippy::too_many_arguments)]
    fn add_child(
        &mut self,
        idx: u32,
        child: Vec<u16>,
        bi: usize,
        d: usize,
        box_at: &mut [bool],
        depth: usize,
        (g, tagged, phase, score, frozen_early, slb0): (u32, u32, u16, i64, bool, u32),
        inherit: bool,
    ) {
        let board = self.board;
        let to = child[bi];
        let from = board.neighbors[to as usize][OPP[d]];
        // Every box must still have a goal of its own.
        let unmatched = crate::matching::min_cost_matching(child.len(), board.goals.len(), |i, j| {
            let dist = board.goal_dist[j][child[i] as usize];
            (dist != INF).then_some(dist)
        })
        .is_none();
        if unmatched {
            return;
        }
        let (creach, cnorm) = flood(board, box_at, from);
        if self.matches_pattern(to, box_at, &creach, from) {
            return;
        }
        let mut sorted = child.clone();
        sorted.sort_unstable();
        let key = (sorted.into_boxed_slice(), cnorm);
        // A known position is taken over by a strictly shorter path while it
        // is not expanded yet.
        let known = self.tt.get(&key).copied();
        if known.is_some_and(|k| self.nodes[k as usize].expanded || self.nodes[k as usize].g <= g + 1) {
            return;
        }
        let slb = slb0 - board.min_goal_dist[from as usize] + board.min_goal_dist[to as usize];
        let base = (g + 1) as i64 - tagged as i64 + slb as i64 + OFFSET;
        let (cphase, mut cscore, approaching) = self.ph.score(phase, base, from, to, box_at);
        // A box frozen on a goal that a later phase fills costs half the
        // lower bound; the flag follows the path until no such box is left.
        let mut cfrozen = false;
        if cphase > 0 && (frozen_early || self.ph.at[to as usize].iter().any(|&k| !self.ph.entries[k].parking && self.ph.entries[k].phase < cphase)) {
            let n = self.ph.frozen_early(cphase, board, box_at, &mut self.freeze);
            if n > 0 {
                cfrozen = true;
                cscore += n * slb as i64 / 2;
            }
        }
        if inherit && cscore > score {
            cscore = score;
        }
        let node = Node {
            boxes: child.into_boxed_slice(),
            player: from,
            g: g + 1,
            tagged: tagged + approaching as u32,
            phase: cphase,
            score: cscore,
            frozen_early: cfrozen,
            expanded: false,
            queued: false,
            parent: idx,
            from,
            dir: d as u8,
        };
        let solved = node.boxes.iter().all(|&s| board.is_goal[s as usize]);
        let cidx = match known {
            Some(k) => {
                self.unqueue(k);
                self.nodes[k as usize] = node;
                k
            }
            None => {
                self.nodes.push(node);
                self.tt.insert(key, self.nodes.len() as u32 - 1);
                self.nodes.len() as u32 - 1
            }
        };
        if solved {
            self.solved = Some(cidx);
        } else if cscore <= score && cscore as usize <= self.open_min() && depth < MAX_DIVE {
            box_at[to as usize] = false;
            box_at[from as usize] = true;
            self.expand(cidx, depth + 1);
            box_at[from as usize] = false;
            box_at[to as usize] = true;
        } else {
            self.push_open(cidx, cscore);
        }
    }
}

impl Search<'_> {
    /// Look for a corral whose boxes alone can never all reach goals; if
    /// one is found, keep it as a pattern and report the node dead.
    fn learn_dead_corral(&mut self, box_at: &[bool], reach: &[bool], player: u16) -> bool {
        let board = self.board;
        let n = board.num_squares;
        let mut seen = vec![false; n];
        for seed in 0..n {
            if seen[seed] || box_at[seed] || reach[seed] {
                continue;
            }
            // An unreachable area and the boxes fencing it.
            let mut area = vec![false; n];
            let mut fence: Vec<u16> = Vec::new();
            let mut stack = vec![seed as u16];
            seen[seed] = true;
            area[seed] = true;
            let mut inside = seed as u16;
            while let Some(q) = stack.pop() {
                inside = inside.min(q);
                for &m in &board.neighbors[q as usize] {
                    if m == NONE || area[m as usize] {
                        continue;
                    }
                    area[m as usize] = true;
                    if box_at[m as usize] {
                        fence.push(m);
                    } else {
                        seen[m as usize] = true;
                        stack.push(m);
                    }
                }
            }
            if fence.is_empty() || fence.len() > LEARN_BOXES || fence.iter().all(|&b| board.is_goal[b as usize]) {
                continue;
            }
            fence.sort_unstable();
            let mut only = vec![false; n];
            for &b in &fence {
                only[b as usize] = true;
            }
            let norm = flood(board, &only, player).1;
            let key = (fence.clone().into_boxed_slice(), norm);
            let dead = match self.proved.get(&key) {
                Some(&v) => v,
                None => {
                    let v = never_all_home(board, &fence, player, &area, PROOF_STATES);
                    self.proved.insert(key, v);
                    v
                }
            };
            if dead {
                let id = self.patterns.len() as u32;
                for &b in &fence {
                    self.patterns_at[b as usize].push(id);
                }
                self.patterns.push((fence.into_boxed_slice(), norm, inside));
                return true;
            }
        }
        false
    }

    /// Does a learned dead corral appear around the box just pushed to
    /// `to`? Its boxes must all be present, the player shut out of it, and
    /// the player's region, counting only the pattern's boxes, the one the
    /// proof started from.
    fn matches_pattern(&self, to: u16, box_at: &[bool], reach: &[bool], player: u16) -> bool {
        self.patterns_at[to as usize].iter().any(|&id| {
            let (boxes, norm, inside) = &self.patterns[id as usize];
            boxes.iter().all(|&b| box_at[b as usize])
                && !reach[*inside as usize]
                && !box_at[*inside as usize]
                && {
                    let mut only = vec![false; self.board.num_squares];
                    for &b in boxes.iter() {
                        only[b as usize] = true;
                    }
                    flood(self.board, &only, player).1 == *norm
                }
        })
    }
}

/// Can `boxes` alone (no other box on the board), with the player starting
/// at `player`, never all stand on goals at once? A box pushed out of
/// `area` (the corral and its fence) escapes and is dropped — optimistic,
/// so a "yes" (true) is a proof. Removing boxes never makes a position
/// harder, so the boxes stay dead in any position that holds them with the
/// player in the same region. False also when the budget runs out.
fn never_all_home(board: &Board, boxes: &[u16], player: u16, area: &[bool], budget: usize) -> bool {
    let n = board.num_squares;
    let mut at = vec![false; n];
    let mut seen: rustc_hash::FxHashSet<(Vec<u16>, u16)> = rustc_hash::FxHashSet::default();
    let mut stack: Vec<(Vec<u16>, u16)> = Vec::new();
    let start = (boxes.to_vec(), {
        for &b in boxes {
            at[b as usize] = true;
        }
        let r = flood(board, &at, player).1;
        for &b in boxes {
            at[b as usize] = false;
        }
        r
    });
    seen.insert(start.clone());
    stack.push(start);
    while let Some((bx, pl)) = stack.pop() {
        if bx.iter().all(|&b| board.is_goal[b as usize]) {
            return false;
        }
        if seen.len() > budget {
            return false;
        }
        for &b in &bx {
            at[b as usize] = true;
        }
        let (reach, _) = flood(board, &at, pl);
        for (i, &b) in bx.iter().enumerate() {
            for d in 0..4 {
                let to = board.neighbors[b as usize][d];
                let behind = board.neighbors[b as usize][OPP[d]];
                if to == NONE || behind == NONE || at[to as usize] || board.dead[to as usize] || !reach[behind as usize] {
                    continue;
                }
                let mut nb = bx.clone();
                if area[to as usize] {
                    nb[i] = to;
                } else {
                    nb.remove(i);
                }
                if nb.is_empty() {
                    return false;
                }
                nb.sort_unstable();
                at[b as usize] = false;
                for &x in &nb {
                    at[x as usize] = true;
                }
                let norm = flood(board, &at, b).1;
                for &x in &nb {
                    at[x as usize] = false;
                }
                for &x in &bx {
                    at[x as usize] = true;
                }
                if seen.insert((nb.clone(), norm)) {
                    stack.push((nb, norm));
                }
            }
        }
        for &b in &bx {
            at[b as usize] = false;
        }
    }
    true
}

/// A tunnel push (YASS): the box left a square walled on both sides across
/// the push for a non-goal square where it cannot move across either. It
/// may as well keep going, so no other box needs to move first.
fn tunnel(board: &Board, from: u16, d: usize) -> bool {
    let to = board.neighbors[from as usize][d];
    let across = if d < 2 { [2, 3] } else { [0, 1] };
    let wall = |q: u16, e: usize| board.neighbors[q as usize][e] == NONE;
    let dead = |q: u16, e: usize| board.dead[board.neighbors[q as usize][e] as usize];
    !board.is_goal[to as usize]
        && across.iter().all(|&e| wall(from, e))
        && (across.iter().any(|&e| wall(to, e)) || across.iter().all(|&e| dead(to, e)))
}

/// Player region from `start`: (reachable flags, smallest square).
fn flood(board: &Board, box_at: &[bool], start: u16) -> (Vec<bool>, u16) {
    let mut reach = vec![false; board.num_squares];
    let mut stack = vec![start];
    reach[start as usize] = true;
    let mut min = start;
    while let Some(s) = stack.pop() {
        min = min.min(s);
        for &n in &board.neighbors[s as usize] {
            if n != NONE && !reach[n as usize] && !box_at[n as usize] {
                reach[n as usize] = true;
                stack.push(n);
            }
        }
    }
    (reach, min)
}

/// Push distance from every square to `target` for a single box, counting
/// only pushes the player can line up for (the player's side of a box is
/// tracked by connected component of the board without the box square).
fn aware_distances(neighbors: &[[u16; 4]], target: u16) -> Vec<u32> {
    let n = neighbors.len();
    // side[q][d]: component label of q's neighbour in direction d when q is
    // blocked, so two sides with one label are mutually reachable.
    let mut side = vec![[u8::MAX; 4]; n];
    let mut stamp = vec![usize::MAX; n];
    let mut stack = Vec::new();
    for q in 0..n {
        for d in 0..4 {
            let start = neighbors[q][d];
            if start == NONE || side[q][d] != u8::MAX {
                continue;
            }
            let run = q * 4 + d;
            stamp[start as usize] = run;
            stack.push(start);
            while let Some(sq) = stack.pop() {
                for &m in &neighbors[sq as usize] {
                    if m != NONE && m as usize != q && stamp[m as usize] != run {
                        stamp[m as usize] = run;
                        stack.push(m);
                    }
                }
            }
            for e in d..4 {
                let s = neighbors[q][e];
                if s != NONE && stamp[s as usize] == run {
                    side[q][e] = d as u8;
                }
            }
        }
    }
    // Backward search over (box square, player side): a push into `dest`
    // along OPP[e] came from q = dest+e with the player behind q, and leaves
    // the player on q.
    let mut dist = vec![INF; n * 4];
    let mut queue = VecDeque::new();
    for c in 0..4u8 {
        if side[target as usize].contains(&c) {
            dist[target as usize * 4 + c as usize] = 0;
            queue.push_back((target, c));
        }
    }
    while let Some((dest, c)) = queue.pop_front() {
        let dd = dist[dest as usize * 4 + c as usize];
        for e in 0..4 {
            if side[dest as usize][e] != c {
                continue;
            }
            let q = neighbors[dest as usize][e];
            if q == NONE || neighbors[q as usize][e] == NONE {
                continue;
            }
            let k = q as usize * 4 + side[q as usize][e] as usize;
            if dist[k] == INF {
                dist[k] = dd + 1;
                queue.push_back((q, side[q as usize][e]));
            }
        }
    }
    (0..n).map(|q| dist[q * 4..q * 4 + 4].iter().copied().min().unwrap()).collect()
}

pub fn solve(board: &Board, opts: &Options) -> Outcome {
    let start = Instant::now();
    let exhausted = |start: Instant| Outcome::Exhausted { stats: Stats { time: start.elapsed(), ..Stats::default() } };
    // Where YASS uses no packing order (few connected goals), step aside.
    let Some(order) = crate::yassorder::compute(board, opts.time_limit) else { return exhausted(start) };
    let ph = Phases::new(board, &order);
    let mut s = Search {
        board,
        ph,
        nodes: Vec::new(),
        tt: FxHashMap::default(),
        buckets: Vec::new(),
        open_min: usize::MAX,
        live: Vec::new(),
        rover: 0,
        generated: 0,
        freeze: FreezeChecker::new(board),
        dead_sets: DeadSetTables::new(board, Direction::Forward),
        corral: YassCorral::new(board),
        patterns: Vec::new(),
        patterns_at: vec![Vec::new(); board.num_squares],
        proved: FxHashMap::default(),
        start,
        opts,
        solved: None,
    };
    let mut box_at = vec![false; board.num_squares];
    for &b in &board.start_boxes {
        box_at[b as usize] = true;
    }
    let root_key = (board.start_boxes.clone().into_boxed_slice(), flood(board, &box_at, board.start_player).1);
    s.nodes.push(Node {
        boxes: board.start_boxes.clone().into_boxed_slice(),
        player: board.start_player,
        g: 0,
        tagged: 0,
        phase: s.ph.settle(s.ph.count, &box_at),
        score: board.start_boxes.iter().map(|&b| board.min_goal_dist[b as usize] as i64).sum(),
        frozen_early: false,
        expanded: false,
        queued: false,
        parent: NO_PARENT,
        from: 0,
        dir: 0,
    });
    s.tt.insert(root_key, 0);
    s.expand(0, 0);
    while s.solved.is_none() && !s.out_of_time() {
        let Some(idx) = s.pop_open() else { break };
        s.expand(idx, 0);
    }
    let stats = Stats { expanded: s.nodes.len() as u64, time: start.elapsed(), ..Stats::default() };
    let Some(mut idx) = s.solved else { return Outcome::Exhausted { stats } };
    let mut pushes = Vec::new();
    while s.nodes[idx as usize].parent != NO_PARENT {
        let n = &s.nodes[idx as usize];
        pushes.push((n.from, n.dir));
        idx = n.parent;
    }
    pushes.reverse();
    Outcome::Solved { pushes, stats }
}
