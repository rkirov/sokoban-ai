//! Small box-set deadlock tables (pairs and triples), exact by retrograde
//! search.
//!
//! Take any K boxes alone on the board (every other box removed) with the
//! player in some region. If even they cannot all reach goals, the full
//! position is dead too — removing boxes only makes a position easier, and
//! every goal stays available. Which (box set, player region) states CAN be
//! finished is computed once per level by breadth-first PULLS from every
//! state with all K boxes on goals; everything not reached is a deadlock.
//!
//! Small dense levels are full of such deadlocks that the freeze test and
//! the matching bound miss (two boxes adjacent in a one-wide corridor, a box
//! stuck behind another in a goal-less dead end, three boxes jamming a
//! doorway, ...). Each costs the search a whole wasted subtree. Measured:
//! pairs cut A* nodes 16-20% on Microban with identical optimal solutions.
//!
//! State = (sorted squares, player region of the floor minus those squares).
//! Most sets leave the floor in one piece; per-square region labels are
//! stored only for the sets that split it.
//!
//! The mirror-image table (seeded from start squares, expanded by pushes)
//! serves backward (pull) searches: box sets that could never have come
//! from the start position.

use crate::level::{Board, NONE, OPP};
use rustc_hash::FxHashMap;

pub struct DeadSets {
    k: usize,
    n: usize,
    /// dead[code] bit z: the set with the player in region z is a deadlock.
    /// Regions beyond the 8th are never marked (conservative).
    dead: Vec<u8>,
    /// Region label per square, for sets that split the floor.
    labels: FxHashMap<u32, Box<[u8]>>,
}

/// Which search the table serves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Forward (push) search: sets that can reach goals.
    Forward,
    /// Backward (pull) search: sets that can be pulled back onto start
    /// squares, i.e. that the start position can push boxes into.
    Backward,
}

/// Level-size caps: tables cost O(squares^(k+1)) to build and O(squares^k)
/// bytes of memory. (Quadruples were measured too: 36% fewer A* nodes on
/// SokHard but no more levels solved, so they are not built.)
fn max_squares(k: usize) -> usize {
    if k == 2 { 400 } else { 120 }
}

impl DeadSets {
    /// Deadlock table for sets of `k` (2 or 3) boxes, or None when the
    /// level is too large (or has too few goals / start squares).
    pub fn compute(board: &Board, k: usize, dir: Direction) -> Option<Self> {
        assert!(k == 2 || k == 3);
        let n = board.num_squares;
        let (seeds, excluded) = match dir {
            Direction::Forward => (&board.goals, &board.dead),
            Direction::Backward => (&board.start_boxes, &board.backward_dead),
        };
        if n > max_squares(k) || seeds.len() < k {
            return None;
        }
        let mut db = DeadSets { k, n, dead: vec![0; n.pow(k as u32)], labels: FxHashMap::default() };

        // Region count (and labels, when > 1) of the floor minus each set.
        let mut zone_count = vec![1u8; n.pow(k as u32)];
        let mut label = vec![u8::MAX; n];
        let mut stack: Vec<u16> = Vec::new();
        let live: Vec<u16> = (0..n as u16).filter(|&s| !excluded[s as usize]).collect();
        for_each_set(&live, k, &mut |set| {
            label.iter_mut().for_each(|l| *l = u8::MAX);
            for &s in set {
                label[s as usize] = u8::MAX - 1;
            }
            let mut zones = 0u8;
            for s in 0..n {
                if label[s] != u8::MAX {
                    continue;
                }
                label[s] = zones;
                stack.push(s as u16);
                while let Some(q) = stack.pop() {
                    for &nb in &board.neighbors[q as usize] {
                        if nb != NONE && label[nb as usize] == u8::MAX {
                            label[nb as usize] = zones;
                            stack.push(nb);
                        }
                    }
                }
                zones = zones.saturating_add(1);
            }
            let code = db.code(set);
            zone_count[code] = zones;
            if zones > 1 {
                db.labels.insert(code as u32, label.clone().into_boxed_slice());
            }
        });

        // Retrograde BFS: reached[code] bit z = can be finished.
        let mut reached = vec![0u8; n.pow(k as u32)];
        let mut queue: Vec<([u16; 3], u8)> = Vec::new();
        for_each_set(seeds, k, &mut |set| {
            let mut s = [0u16; 3];
            s[..k].copy_from_slice(set);
            s[..k].sort_unstable();
            let code = db.code(&s[..k]);
            for z in 0..zone_count[code].min(8) {
                if reached[code] & (1 << z) == 0 {
                    reached[code] |= 1 << z;
                    queue.push((s, z));
                }
            }
        });
        let mut head = 0;
        while head < queue.len() {
            let (set, z) = queue[head];
            head += 1;
            let set = &set[..k];
            // Move one box one square, retrograde to the search served:
            // Forward tables PULL (the player stands on the destination, in
            // region z, and backs away); Backward tables PUSH (the player
            // stands behind the box, in region z, and ends on its square).
            for (mi, &mover) in set.iter().enumerate() {
                for d in 0..4 {
                    let to = board.neighbors[mover as usize][d];
                    if to == NONE || set.contains(&to) || excluded[to as usize] {
                        continue;
                    }
                    let (stand, player_after) = match dir {
                        Direction::Forward => (to, board.neighbors[to as usize][d]),
                        Direction::Backward => (board.neighbors[mover as usize][OPP[d]], mover),
                    };
                    // The player's squares must be free of the OTHER boxes (in a
                    // push the player ends on the mover's own square).
                    let blocked = |sq: u16| sq == NONE || set.iter().enumerate().any(|(i, &x)| i != mi && x == sq);
                    if blocked(stand) || blocked(player_after) {
                        continue;
                    }
                    if db.zone(set, stand) != z {
                        continue;
                    }
                    let mut next = [0u16; 3];
                    next[..k].copy_from_slice(set);
                    next[mi] = to;
                    next[..k].sort_unstable();
                    let nz = db.zone(&next[..k], player_after);
                    let code = db.code(&next[..k]);
                    if nz < 8 && reached[code] & (1 << nz) == 0 {
                        reached[code] |= 1 << nz;
                        queue.push((next, nz));
                    }
                }
            }
        }

        for_each_set(&live, k, &mut |set| {
            let code = db.code(set);
            let zones = zone_count[code].min(8);
            let all = if zones >= 8 { u8::MAX } else { ((1u16 << zones) - 1) as u8 };
            db.dead[code] = all & !reached[code];
        });
        Some(db)
    }

    /// Index of a sorted set.
    fn code(&self, set: &[u16]) -> usize {
        set.iter().fold(0, |acc, &s| acc * self.n + s as usize)
    }

    /// Player region of `sq` in the floor minus the sorted `set`.
    fn zone(&self, set: &[u16], sq: u16) -> u8 {
        match self.labels.get(&(self.code(set) as u32)) {
            Some(l) => l[sq as usize],
            None => 0,
        }
    }

    /// Is the set of boxes on `squares` (any order) dead with the player on
    /// `player`?
    pub fn is_dead(&self, squares: &[u16], player: u16) -> bool {
        let mut set = [0u16; 3];
        set[..self.k].copy_from_slice(squares);
        set[..self.k].sort_unstable();
        let set = &set[..self.k];
        let mask = self.dead[self.code(set)];
        if mask == 0 {
            return false;
        }
        let z = self.zone(set, player);
        z < 8 && mask & (1 << z) != 0
    }

    /// After box `moved` of `boxes` (the list before the move) arrived on
    /// `to` with the player on `player`, does it form a dead set with the
    /// other boxes?
    pub fn moved_box_dead(&self, boxes: &[u16], moved: usize, to: u16, player: u16) -> bool {
        let others = || boxes.iter().enumerate().filter(move |&(i, _)| i != moved).map(|(_, &b)| b);
        match self.k {
            2 => others().any(|b| self.is_dead(&[to, b], player)),
            _ => others().enumerate().any(|(i, b)| others().skip(i + 1).any(|c| self.is_dead(&[to, b, c], player))),
        }
    }
}

/// Call `f` on every k-subset of `squares`, in increasing order.
fn for_each_set(squares: &[u16], k: usize, f: &mut impl FnMut(&[u16])) {
    fn rec(squares: &[u16], k: usize, start: usize, set: &mut Vec<u16>, f: &mut impl FnMut(&[u16])) {
        if set.len() == k {
            f(set);
            return;
        }
        for i in start..squares.len() {
            set.push(squares[i]);
            rec(squares, k, i + 1, set, f);
            set.pop();
        }
    }
    rec(squares, k, 0, &mut Vec::with_capacity(k), f);
}

/// The deadlock tables a search consults after moving a box: pairs, and
/// triples on levels small enough to afford them.
pub struct DeadSetTables {
    tables: Vec<DeadSets>,
}

impl DeadSetTables {
    pub fn new(board: &Board, dir: Direction) -> Self {
        DeadSetTables { tables: [2, 3].iter().filter_map(|&k| DeadSets::compute(board, k, dir)).collect() }
    }

    /// Does box `moved` of `boxes` (the list before the move), now on `to`
    /// with the player on `player`, form a dead set with the other boxes?
    pub fn moved_box_dead(&self, boxes: &[u16], moved: usize, to: u16, player: u16) -> bool {
        self.tables.iter().any(|t| t.moved_box_dead(boxes, moved, to, player))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{parse_collection, Level};
    use crate::solver::{solve, Options, Outcome};

    fn board(text: &str) -> Board {
        Board::from_level(&parse_collection(text)[0]).unwrap()
    }

    #[test]
    fn two_boxes_in_a_one_wide_corridor_are_dead() {
        // Adjacent boxes in a one-wide corridor: the rear box cannot move
        // until the front one does, and the player can never reach the
        // front one's far side from behind — dead from either side.
        let b = board("########\n#@ $$..#\n########");
        let db = DeadSets::compute(&b, 2, Direction::Forward).unwrap();
        let sq = |x: usize| b.sq_index[b.width + x];
        assert!(db.is_dead(&[sq(3), sq(4)], sq(1)));
        assert!(db.is_dead(&[sq(3), sq(4)], sq(5)));
    }

    #[test]
    fn two_lanes_are_fine() {
        let b = board("########\n#@ $  .#\n#  $  .#\n########");
        let db = DeadSets::compute(&b, 2, Direction::Forward).unwrap();
        let sq = |x: usize, y: usize| b.sq_index[y * b.width + x];
        assert!(!db.is_dead(&[sq(3, 1), sq(3, 2)], sq(1, 1)));
    }

    /// Sub-level with only the given boxes and player (same walls, every
    /// goal), solved exhaustively; None if the solve hit its node limit.
    fn solvable(b: &Board, boxes: &[u16], player: u16) -> Option<bool> {
        let mut rows: Vec<Vec<u8>> = vec![vec![b'#'; b.width]; b.height];
        for sq in 0..b.num_squares {
            let cell = b.sq_pos[sq];
            rows[cell / b.width][cell % b.width] = if b.is_goal[sq] { b'.' } else { b' ' };
        }
        let mut place = |sq: u16, plain: u8, on_goal: u8| {
            let cell = b.sq_pos[sq as usize];
            let c = &mut rows[cell / b.width][cell % b.width];
            *c = if *c == b'.' { on_goal } else { plain };
        };
        for &x in boxes {
            place(x, b'$', b'*');
        }
        place(player, b'@', b'+');
        let sub = Board::from_level(&Level { name: "set".into(), rows }).unwrap();
        match solve(&sub, &Options { max_nodes: 200_000, ..Options::default() }) {
            Outcome::Solved { .. } => Some(true),
            Outcome::Unsolvable { .. } => Some(false),
            Outcome::Exhausted { .. } => None,
        }
    }

    /// The tables must agree with exhaustive solves of the K-box sub-level
    /// for random sets and player squares of real levels.
    #[test]
    fn agree_with_exhaustive_search() {
        let mut rng = 0x1234_5678_9abc_def1u64;
        let mut next = move |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n as u64) as usize
        };
        for k in [2, 3] {
            let mut checked = (0, 0);
            for file in ["levels/microban1.txt", "levels/microban3.txt"] {
                let text = std::fs::read_to_string(file).unwrap();
                for lvl in parse_collection(&text).iter().step_by(7) {
                    let b = Board::from_level(lvl).unwrap();
                    let Some(db) = DeadSets::compute(&b, k, Direction::Forward) else { continue };
                    let live: Vec<u16> = (0..b.num_squares as u16).filter(|&s| !b.dead[s as usize]).collect();
                    for _ in 0..30 {
                        let set: Vec<u16> = (0..k).map(|_| live[next(live.len())]).collect();
                        let p = next(b.num_squares) as u16;
                        let mut uniq = set.clone();
                        uniq.push(p);
                        uniq.sort_unstable();
                        uniq.dedup();
                        if uniq.len() != k + 1 {
                            continue;
                        }
                        let Some(ok) = solvable(&b, &set, p) else { continue };
                        assert_eq!(db.is_dead(&set, p), !ok, "k={k} {} set {set:?} player {p}", lvl.name);
                        if ok { checked.0 += 1 } else { checked.1 += 1 }
                    }
                }
            }
            assert!(checked.0 > 20 && checked.1 > 20, "k={k}: {checked:?}");
        }
    }
}

#[cfg(test)]
mod solution_tests {
    use super::*;
    use crate::level::parse_collection;
    use crate::solver::{solve, Options, Outcome};

    /// Ground truth for both directions: every position along a real
    /// solution is reachable from the start AND can reach the goal, so every
    /// box subset of it (with the actual player square) must be alive in the
    /// forward table and in the backward table. (A refactoring once rejected
    /// every backward push transition — the player ends on the mover's own
    /// square — and the backward search reported Microban I #36 unsolvable.)
    #[test]
    fn solution_positions_are_alive_in_both_directions() {
        let mut checked = 0;
        for file in ["levels/microban1.txt", "levels/microban2.txt"] {
            let text = std::fs::read_to_string(file).unwrap();
            for lvl in parse_collection(&text).iter().step_by(6) {
                let b = Board::from_level(lvl).unwrap();
                let opts = Options { max_nodes: 300_000, ..Options::default() };
                let Outcome::Solved { pushes, .. } = solve(&b, &opts) else { continue };
                let tables: Vec<(DeadSets, &str)> = [2, 3]
                    .into_iter()
                    .flat_map(|k| [(k, Direction::Forward), (k, Direction::Backward)])
                    .filter_map(|(k, dir)| DeadSets::compute(&b, k, dir).map(|t| (t, if dir == Direction::Forward { "fwd" } else { "bwd" })))
                    .collect();
                let mut boxes = b.start_boxes.clone();
                let mut player = b.start_player;
                for step in 0..=pushes.len() {
                    for (t, name) in &tables {
                        for_each_set(&boxes, t.k, &mut |set| {
                            assert!(!t.is_dead(set, player), "{} {name} k={} step {step} {set:?}", lvl.name, t.k);
                            checked += 1;
                        });
                    }
                    if step == pushes.len() {
                        break;
                    }
                    let (from, d) = pushes[step];
                    let bi = boxes.iter().position(|&x| x == from).unwrap();
                    boxes[bi] = b.neighbors[from as usize][d as usize];
                    player = from;
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
    }
}
