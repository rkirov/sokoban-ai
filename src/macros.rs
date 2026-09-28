//! Macro moves: "box from A to B", any number of pushes of one box.
//!
//! With the other boxes held fixed, a single box's reachable destinations
//! come from a breadth-first search over (box square, player side) states.
//! A push moves the box one square away from the player and costs 1; the
//! player walking to another side of the box costs 0. Every state first
//! reached by a push is a distinct successor position (destination square +
//! player region), so one macro move is emitted per such state, carrying
//! its minimal push count.
//!
//! Which sides of square t can the player walk between while the box sits
//! on t? Those in the same connected component of the free squares minus t.
//! One depth-first search per box (the other boxes are fixed, so the free
//! squares don't change while that box moves) answers this for every t via
//! articulation points: a DFS child c of t is cut off from the rest exactly
//! when low[c] >= disc[t]. That is O(squares) per box instead of a flood
//! fill per reached square.
//!
//! Soundness: the macro successors of a position are exactly the positions
//! reachable by pushing one box any number of times, so a search over macro
//! moves reaches every position a unit-push search reaches (a unit push is
//! a one-push macro). Intermediate squares are only required to be live
//! (not dead squares): a box passing through a dead square could never reach
//! a goal afterwards.

use crate::level::{Board, NONE, OPP};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Macro {
    /// Index of the moved box in the position's box list.
    pub box_idx: usize,
    pub from: u16,
    pub to: u16,
    /// Square the player stands on after the final push (next to `to`).
    pub player: u16,
    /// Minimal number of pushes for this box path.
    pub pushes: u32,
    /// Free-space regions after the move (0 unless requested).
    pub regions: u32,
}

/// The parent position's free-space regions, for computing each move's
/// region count in O(1): `label[sq]` identifies the region of every free
/// square, `count` is the number of regions.
pub struct Regions<'a> {
    pub label: &'a [u32],
    pub count: u32,
}

/// Reusable scratch space for macro generation.
pub struct MacroGen {
    /// visited[s * 4 + d]: stamp when (box on s, player on side d) was reached.
    visited: Vec<u32>,
    /// Push distance for each (square, side) state (valid when visited).
    dist: Vec<u32>,
    /// Predecessor state for path reconstruction (u32::MAX = root).
    prev: Vec<u32>,
    stamp: u32,
    queue: Vec<u32>,
    /// DFS over the free squares (moving box removed): discovery time,
    /// low-link, subtree size and parent; `dfs_mark == dfs_stamp` marks the
    /// squares visited by the current DFS.
    disc: Vec<u32>,
    low: Vec<u32>,
    size: Vec<u32>,
    parent: Vec<u16>,
    dfs_mark: Vec<u32>,
    dfs_stamp: u32,
    dfs_stack: Vec<(u16, u8)>,
}

impl MacroGen {
    pub fn new(board: &Board) -> Self {
        let n = board.num_squares;
        MacroGen {
            visited: vec![0; n * 4],
            dist: vec![0; n * 4],
            prev: vec![u32::MAX; n * 4],
            stamp: 0,
            queue: Vec::new(),
            disc: vec![0; n],
            low: vec![0; n],
            size: vec![0; n],
            parent: vec![NONE; n],
            dfs_mark: vec![0; n],
            dfs_stamp: 0,
            dfs_stack: Vec::new(),
        }
    }

    /// Iterative DFS from `root` over free squares (`!box_at`), recording
    /// discovery times, low-links, subtree sizes and parents.
    fn dfs(&mut self, board: &Board, box_at: &[bool], root: u16) {
        self.dfs_stamp += 1;
        let mark = self.dfs_stamp;
        let mut time = 0u32;
        self.dfs_mark[root as usize] = mark;
        self.disc[root as usize] = time;
        self.low[root as usize] = time;
        self.parent[root as usize] = NONE;
        time += 1;
        self.dfs_stack.clear();
        self.dfs_stack.push((root, 0));
        while let Some(&mut (u, ref mut next)) = self.dfs_stack.last_mut() {
            if (*next as usize) < 4 {
                let v = board.neighbors[u as usize][*next as usize];
                *next += 1;
                if v == NONE || box_at[v as usize] {
                    continue;
                }
                if self.dfs_mark[v as usize] != mark {
                    self.dfs_mark[v as usize] = mark;
                    self.disc[v as usize] = time;
                    self.low[v as usize] = time;
                    self.parent[v as usize] = u;
                    time += 1;
                    self.dfs_stack.push((v, 0));
                } else if v != self.parent[u as usize] {
                    self.low[u as usize] = self.low[u as usize].min(self.disc[v as usize]);
                }
            } else {
                self.dfs_stack.pop();
                self.size[u as usize] = time - self.disc[u as usize];
                let p = self.parent[u as usize];
                if p != NONE {
                    self.low[p as usize] = self.low[p as usize].min(self.low[u as usize]);
                }
            }
        }
    }

    /// Region label of neighbor `n` of `t` in the free squares minus `t`:
    /// the DFS child of t whose subtree holds n if that subtree is cut off
    /// (low >= disc[t]), else NONE for "t's parent side". Two sides of t
    /// share a player region iff their labels are equal.
    fn side_region(&self, board: &Board, t: u16, n: u16) -> u16 {
        let (dt, dn) = (self.disc[t as usize], self.disc[n as usize]);
        for &c in &board.neighbors[t as usize] {
            if c == NONE || self.dfs_mark[c as usize] != self.dfs_stamp || self.parent[c as usize] != t {
                continue;
            }
            let dc = self.disc[c as usize];
            if dc <= dn && dn < dc + self.size[c as usize] {
                return if self.low[c as usize] >= dt { c } else { NONE };
            }
        }
        NONE
    }

    /// Number of pieces the current DFS component splits into when `t` is
    /// occupied: its cut-off DFS child subtrees, plus the parent side unless
    /// `t` is the DFS root.
    fn pieces(&self, board: &Board, t: u16) -> u32 {
        let dt = self.disc[t as usize];
        let cut = board.neighbors[t as usize]
            .iter()
            .filter(|&&c| {
                c != NONE
                    && self.dfs_mark[c as usize] == self.dfs_stamp
                    && self.parent[c as usize] == t
                    && self.low[c as usize] >= dt
            })
            .count() as u32;
        cut + (self.parent[t as usize] != NONE) as u32
    }

    /// Fill `regions` for the moves `out[start..]` of the box just searched
    /// (its DFS is still current). Lifting the box off `from` merges the
    /// parent regions around it into the DFS component; every other region
    /// is untouched, and occupying `to` splits the component into `pieces`.
    fn fill_regions(&self, board: &Board, from: u16, parent: &Regions, out: &mut [Macro]) {
        let mut around = [u32::MAX; 4];
        for (i, &n) in board.neighbors[from as usize].iter().enumerate() {
            if n != NONE && self.dfs_mark[n as usize] == self.dfs_stamp && n != from {
                around[i] = parent.label[n as usize];
            }
        }
        let mut distinct = 0;
        for i in 0..4 {
            if around[i] != u32::MAX && !around[..i].contains(&around[i]) {
                distinct += 1;
            }
        }
        let others = parent.count - distinct;
        for m in out {
            m.regions = others + self.pieces(board, m.to);
        }
    }

    /// Mark every side of `t` in the same player region as the side the
    /// player stands on (`side`) as reached, queueing them.
    fn mark_sides(&mut self, board: &Board, box_at: &[bool], t: u16, side: usize, d: u32, pred: u32) {
        let label = self.side_region(board, t, board.neighbors[t as usize][side]);
        for other in 0..4 {
            let n = board.neighbors[t as usize][other];
            if n == NONE || box_at[n as usize] {
                continue;
            }
            if other != side && self.side_region(board, t, n) != label {
                continue;
            }
            let st = t as usize * 4 + other;
            if self.visited[st] != self.stamp {
                self.visited[st] = self.stamp;
                self.dist[st] = d;
                self.prev[st] = pred;
                self.queue.push(st as u32);
            }
        }
    }

    /// Single-box search from `from` (the box on `from` moves, all others in
    /// `box_at` stay; `box_at` is restored before returning). Root states are
    /// the sides of `from` for which `root_side(side)` holds. Calls
    /// `emit(to, player, moves, state)` once per distinct successor
    /// (destination, player region); `emit` returns true to stop early.
    ///
    /// Push: from (s, side) the box moves away from the player to
    /// `s - side`, the player ends on `s`. Pull: the box moves onto the
    /// player's square `s + side`, the player backs up one more square.
    fn search(
        &mut self,
        board: &Board,
        box_at: &mut [bool],
        from: u16,
        kind: Kind,
        root_side: impl Fn(usize) -> bool,
        mut emit: impl FnMut(u16, u16, u32, u32) -> bool,
    ) {
        self.stamp += 1;
        self.queue.clear();
        box_at[from as usize] = false;
        self.dfs(board, box_at, from);
        for side in 0..4 {
            let n = board.neighbors[from as usize][side];
            if n != NONE && root_side(side) {
                let st = from as usize * 4 + side;
                self.visited[st] = self.stamp;
                self.dist[st] = 0;
                self.prev[st] = u32::MAX;
                self.queue.push(st as u32);
            }
        }
        let mut head = 0;
        while head < self.queue.len() {
            let st = self.queue[head] as usize;
            head += 1;
            let (s, side) = ((st / 4) as u16, st % 4);
            let (t, player) = match kind {
                Kind::Push => {
                    let t = board.neighbors[s as usize][OPP[side]];
                    if t == NONE || box_at[t as usize] || board.dead[t as usize] {
                        continue;
                    }
                    (t, s)
                }
                Kind::Pull => {
                    let t = board.neighbors[s as usize][side];
                    let back = board.neighbors[t as usize][side];
                    if back == NONE || box_at[back as usize] {
                        continue;
                    }
                    (t, back)
                }
            };
            let arrive = t as usize * 4 + side; // player on side `side` of t
            if self.visited[arrive] == self.stamp {
                continue;
            }
            // New (square, region) state: mark every side of t in the
            // player's region.
            let d = self.dist[st] + 1;
            self.mark_sides(board, box_at, t, side, d, st as u32);
            if emit(t, player, d, arrive as u32) {
                break;
            }
        }
        box_at[from as usize] = true;
    }

    /// All macro moves of the position. `boxes` is the box list, `box_at`
    /// the matching occupancy (restored before returning), `reach` the
    /// player's current reachability. Destinations are live squares; every
    /// other check (freeze, corral, matching) is the caller's.
    pub fn generate(
        &mut self,
        board: &Board,
        boxes: &[u16],
        box_at: &mut [bool],
        reach: impl Fn(u16) -> bool,
        regions: Option<&Regions>,
        out: &mut Vec<Macro>,
    ) {
        out.clear();
        for (bi, &from) in boxes.iter().enumerate() {
            let start = out.len();
            let root = |side: usize| reach(board.neighbors[from as usize][side]);
            self.search(board, box_at, from, Kind::Push, root, |to, player, pushes, _| {
                out.push(Macro { box_idx: bi, from, to, player, pushes, regions: 0 });
                false
            });
            if let Some(r) = regions {
                self.fill_regions(board, from, r, &mut out[start..]);
            }
        }
    }

    /// Macro moves whose FIRST push is one of `first_pushes` (box square,
    /// direction) — the PI-corral restriction lifted to macros: if some
    /// solution continues with one of those unit pushes, the macro made of
    /// that push plus any further pushes of the same box is among these.
    pub fn generate_restricted(
        &mut self,
        board: &Board,
        boxes: &[u16],
        box_at: &mut [bool],
        first_pushes: &[(u16, u8)],
        regions: Option<&Regions>,
        out: &mut Vec<Macro>,
    ) {
        out.clear();
        for (bi, &from) in boxes.iter().enumerate() {
            let root = |side: usize| first_pushes.contains(&(from, OPP[side] as u8));
            if !(0..4).any(root) {
                continue;
            }
            let start = out.len();
            self.search(board, box_at, from, Kind::Push, root, |to, player, pushes, _| {
                out.push(Macro { box_idx: bi, from, to, player, pushes, regions: 0 });
                false
            });
            if let Some(r) = regions {
                self.fill_regions(board, from, r, &mut out[start..]);
            }
        }
    }

    /// All macro PULLS of the position (backward play): same contract as
    /// `generate`, but boxes are pulled; `Macro::player` is where the player
    /// ends. Destinations are not filtered — the caller decides.
    pub fn generate_pulls(
        &mut self,
        board: &Board,
        boxes: &[u16],
        box_at: &mut [bool],
        reach: impl Fn(u16) -> bool,
        out: &mut Vec<Macro>,
    ) {
        out.clear();
        for (bi, &from) in boxes.iter().enumerate() {
            let root = |side: usize| reach(board.neighbors[from as usize][side]);
            self.search(board, box_at, from, Kind::Pull, root, |to, player, pulls, _| {
                out.push(Macro { box_idx: bi, from, to, player, pushes: pulls, regions: 0 });
                false
            });
        }
    }

    /// Pull search for one box (packing analysis): calls `visit(to)` for
    /// every square the box can be pulled to from `from`, starting with the
    /// player on any side where `root_side` holds; stops when `visit`
    /// returns true. Returns whether it was stopped.
    pub fn pull_search(
        &mut self,
        board: &Board,
        box_at: &mut [bool],
        from: u16,
        root_side: impl Fn(usize) -> bool,
        mut visit: impl FnMut(u16) -> bool,
    ) -> bool {
        let mut stopped = false;
        self.search(board, box_at, from, Kind::Pull, root_side, |to, _, _, _| {
            stopped = visit(to);
            stopped
        });
        stopped
    }

    /// Expand one macro move into unit pushes `(box_from_square, dir)`,
    /// using the same search with predecessor links. `box_at`/`reach`
    /// describe the position before the move.
    pub fn unit_pushes(
        &mut self,
        board: &Board,
        box_at: &mut [bool],
        reach: impl Fn(u16) -> bool,
        m: &Macro,
    ) -> Option<Vec<(u16, u8)>> {
        // The search is deterministic, so the arrival state that generated
        // `m` is found again; follow predecessor links back to the root.
        let mut target = None;
        let root = |side: usize| reach(board.neighbors[m.from as usize][side]);
        self.search(board, box_at, m.from, Kind::Push, root, |to, player, _, state| {
            if to == m.to && player == m.player {
                target = Some(state);
                return true;
            }
            false
        });
        let mut st = target?;
        let mut pushes = Vec::new();
        while self.prev[st as usize] != u32::MAX {
            let p = self.prev[st as usize] as usize;
            // Every link is a push: from state (s, side) the box on s moves
            // away from the player, direction OPP[side].
            let (s, side) = ((p / 4) as u16, p % 4);
            pushes.push((s, OPP[side] as u8));
            st = p as u32;
        }
        pushes.reverse();
        Some(pushes)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Push,
    Pull,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::parse_collection;
    use std::collections::HashSet;

    /// Player region of a position, as its minimum square.
    fn norm(board: &Board, box_at: &[bool], player: u16) -> u16 {
        let mut seen = vec![false; board.num_squares];
        let mut stack = vec![player];
        seen[player as usize] = true;
        let mut m = player;
        while let Some(s) = stack.pop() {
            m = m.min(s);
            for &n in &board.neighbors[s as usize] {
                if n != NONE && !seen[n as usize] && !box_at[n as usize] {
                    seen[n as usize] = true;
                    stack.push(n);
                }
            }
        }
        m
    }

    /// Brute force: every (box square, player region) reachable by unit
    /// pushes of box `from` alone onto live squares, excluding the start.
    fn brute(board: &Board, box_at: &mut [bool], from: u16, player: u16) -> HashSet<(u16, u16)> {
        let mut out = HashSet::new();
        let start = (from, norm(board, box_at, player));
        let mut seen = HashSet::from([start]);
        let mut stack = vec![(from, player)];
        while let Some((b, p)) = stack.pop() {
            box_at[from as usize] = false;
            box_at[b as usize] = true;
            let mut reach = vec![false; board.num_squares];
            let mut st = vec![p];
            reach[p as usize] = true;
            while let Some(s) = st.pop() {
                for &n in &board.neighbors[s as usize] {
                    if n != NONE && !reach[n as usize] && !box_at[n as usize] {
                        reach[n as usize] = true;
                        st.push(n);
                    }
                }
            }
            for d in 0..4 {
                let t = board.neighbors[b as usize][d];
                let behind = board.neighbors[b as usize][OPP[d]];
                if t == NONE || behind == NONE || box_at[t as usize] || board.dead[t as usize] || !reach[behind as usize] {
                    continue;
                }
                box_at[b as usize] = false;
                box_at[t as usize] = true;
                let key = (t, norm(board, box_at, b));
                box_at[t as usize] = false;
                box_at[b as usize] = true;
                if seen.insert(key) {
                    out.insert(key);
                    stack.push((t, b));
                }
            }
            box_at[b as usize] = false;
            box_at[from as usize] = true;
        }
        out
    }

    #[test]
    fn macros_match_brute_force_and_replay() {
        let mut checked = 0;
        for file in ["levels/microban1.txt", "levels/microban4.txt", "levels/xsokoban.txt"] {
            let text = std::fs::read_to_string(file).unwrap();
            for lvl in parse_collection(&text).iter().take(90) {
                let board = Board::from_level(lvl).unwrap();
                let mut box_at = vec![false; board.num_squares];
                for &b in &board.start_boxes {
                    box_at[b as usize] = true;
                }
                let player = board.start_player;
                let pnorm = norm(&board, &box_at, player);
                let mut macro_gen = MacroGen::new(&board);
                let mut moves = Vec::new();
                let reach = {
                    let mut r = vec![false; board.num_squares];
                    let mut st = vec![player];
                    r[player as usize] = true;
                    while let Some(s) = st.pop() {
                        for &n in &board.neighbors[s as usize] {
                            if n != NONE && !r[n as usize] && !box_at[n as usize] {
                                r[n as usize] = true;
                                st.push(n);
                            }
                        }
                    }
                    r
                };
                macro_gen.generate(&board, &board.start_boxes, &mut box_at, |s| reach[s as usize], None, &mut moves);

                for (bi, &from) in board.start_boxes.iter().enumerate() {
                    let expect = brute(&board, &mut box_at, from, player);
                    let mut got = HashSet::new();
                    for m in moves.iter().filter(|m| m.box_idx == bi) {
                        box_at[from as usize] = false;
                        box_at[m.to as usize] = true;
                        let key = (m.to, norm(&board, &box_at, m.player));
                        box_at[m.to as usize] = false;
                        box_at[from as usize] = true;
                        assert!(got.insert(key), "{}: duplicate successor {key:?}", lvl.name);
                    }
                    // The brute force also records returning to `from` in a
                    // new region; the generator emits those too.
                    let expect: HashSet<_> = expect.into_iter().filter(|&k| k != (from, pnorm)).collect();
                    assert_eq!(got, expect, "{} box {from}", lvl.name);
                }

                // Every macro expands into a legal unit-push sequence.
                for m in &moves {
                    let pushes = macro_gen.unit_pushes(&board, &mut box_at, |s| reach[s as usize], m).expect("path");
                    assert_eq!(pushes.len() as u32, m.pushes, "{}", lvl.name);
                    let mut b = m.from;
                    for &(sq, d) in &pushes {
                        assert_eq!(sq, b);
                        b = board.neighbors[b as usize][d as usize];
                    }
                    assert_eq!(b, m.to);
                    let last = pushes.last().unwrap();
                    assert_eq!(last.0, m.player, "player ends on the last push's square");
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "{checked}");
    }

    /// Brute-force region count of a box occupancy.
    fn count_regions(board: &Board, box_at: &[bool], label: &mut [u32]) -> u32 {
        label.iter_mut().for_each(|l| *l = u32::MAX);
        let mut regions = 0;
        for start in 0..board.num_squares {
            if box_at[start] || label[start] != u32::MAX {
                continue;
            }
            let mut stack = vec![start as u16];
            label[start] = regions;
            while let Some(s) = stack.pop() {
                for &n in &board.neighbors[s as usize] {
                    if n != NONE && label[n as usize] == u32::MAX && !box_at[n as usize] {
                        label[n as usize] = regions;
                        stack.push(n);
                    }
                }
            }
            regions += 1;
        }
        regions
    }

    /// Each move's O(1) region count (articulation data) must equal a full
    /// count of the resulting position, on random-walk positions of real
    /// levels.
    #[test]
    fn move_region_counts_match_brute_force() {
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n as u64) as usize
        };
        let mut checked = 0;
        for file in ["levels/microban2.txt", "levels/xsokoban.txt"] {
            let text = std::fs::read_to_string(file).unwrap();
            for lvl in parse_collection(&text).iter().take(40) {
                let board = Board::from_level(lvl).unwrap();
                let mut macro_gen = MacroGen::new(&board);
                let mut boxes = board.start_boxes.clone();
                let mut player = board.start_player;
                let mut label = vec![0u32; board.num_squares];
                let mut moves = Vec::new();
                for _ in 0..15 {
                    let mut box_at = vec![false; board.num_squares];
                    for &b in &boxes {
                        box_at[b as usize] = true;
                    }
                    let mut reach = vec![false; board.num_squares];
                    let mut st = vec![player];
                    reach[player as usize] = true;
                    while let Some(s) = st.pop() {
                        for &n in &board.neighbors[s as usize] {
                            if n != NONE && !reach[n as usize] && !box_at[n as usize] {
                                reach[n as usize] = true;
                                st.push(n);
                            }
                        }
                    }
                    let count = count_regions(&board, &box_at, &mut label);
                    let parent = Regions { label: &label.clone(), count };
                    macro_gen.generate(&board, &boxes, &mut box_at, |s| reach[s as usize], Some(&parent), &mut moves);
                    for m in &moves {
                        box_at[m.from as usize] = false;
                        box_at[m.to as usize] = true;
                        let expect = count_regions(&board, &box_at, &mut label);
                        box_at[m.to as usize] = false;
                        box_at[m.from as usize] = true;
                        assert_eq!(m.regions, expect, "{} move {m:?}", lvl.name);
                        checked += 1;
                    }
                    if moves.is_empty() {
                        break;
                    }
                    let m = moves[next(moves.len())];
                    boxes[m.box_idx] = m.to;
                    player = m.player;
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
    }
}
