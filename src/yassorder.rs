//! YASS's packing-order calculation, re-derived from YASS 2.153
//! (`CalculatePackingOrder` and the board preparation it depends on), so the
//! packing-order search (posearch.rs) can run on its own plans. Diff-tested
//! against an instrumented YASS build.
//!
//! The order is found by taking the solved position apart. Put a box on
//! every goal; then, repeatedly, every box that can be pulled from its goal
//! to a still-unused box starting square is removed from the board, all at
//! once, as one phase. Phase 1 is the first set removed, i.e. the last set
//! filled when the search runs forward. A box qualifies only if:
//! - the player has "seen" every starting square (reached it, or touched a
//!   box on it) since the peeling began;
//! - the pull ends with the player able to walk back to where it was
//!   (except for the last box), and the starting square is not too close to
//!   the goal (a nearby starting square would make the room look easier to
//!   empty than it is) unless it is the endgame;
//! - its pulls cover a large part of the board (25%); a box that can only
//!   wander in a small area is instead *parked*: pulled to a square far from
//!   its goal, which then holds the box as a pseudo goal (a parking entry,
//!   filled before the goal and left again in a later phase).
//!
//! Each removed box claims the starting square that leaves the remaining
//! goals the most choice (max over squares of the min, over open goals that
//! can reach the square, of their count of unused reachable squares). When
//! nothing can be removed, one box is parked (the one whose best parking
//! square is farthest from its goal) and the search recurses, trying a
//! second parking square and other boxes on failure. If that fails
//! everywhere, the fallback just pulls boxes off their goals phase by phase
//! (no starting squares, no parking), seeded with the best partial order.
//!
//! The calculation only runs when at least 9 goals belong to groups of
//! connected goals (YASS's "goal room" test); otherwise there is no order.
//!
//! Squares here live on a padded copy of the grid (one wall cell around the
//! level), scanned row by row like YASS's board; scan order breaks ties.

use crate::level::Board;
use std::time::{Duration, Instant};

/// Direction order matters for tie-breaking: YASS's up, left, down, right.
const UP: usize = 0;
const LEFT: usize = 1;
fn left(d: usize) -> usize {
    (d + 1) & 3
}
fn right(d: usize) -> usize {
    (d + 3) & 3
}

const INF: u32 = u32::MAX;
/// Maximum number of goals plus parking squares (YASS's box-number range).
const MAX_TARGETS: usize = 255;
/// Squares from which a box must come for its goal not to be parked.
const LARGE_AREA_PERCENT: usize = 25;
/// Endgame: the last 15% of the boxes may use any starting square.
const END_GAME_PERCENT: usize = 85;
const MIN_REMOVAL_DISTANCE: usize = 3;
const PARK_ATTEMPTS: u32 = 500 * 1024;
const CONNECTED_GOALS_THRESHOLD: usize = 9;
const MAX_PHASES: u32 = 1000;

/// One entry of the order: a target square (our square index), its phase
/// (counting down, the highest is filled first), whether it is a parking
/// square, and for parking squares the phase in which YASS expects the
/// parked box to move on (0 for goals).
pub type OrderEntry = (u16, u16, bool, u16);

/// YASS's packing order for the level, or None when YASS would not use one
/// (too few connected goals, a malformed level, or no order found within
/// `limit`; YASS allows itself 90 s).
pub fn compute(board: &Board, limit: Duration) -> Option<Vec<OrderEntry>> {
    let mut y = Yass::new(board, Instant::now() + limit)?;
    if y.connected_goals() < CONNECTED_GOALS_THRESHOLD {
        return None;
    }
    y.calculate_tunnels();
    let found = y.calculate(Kind::WithParking) || y.calculate(Kind::PullBoxes);
    if !found {
        return None;
    }
    Some(
        y.order()
            .into_iter()
            .map(|(cell, phase, parking, home)| {
                let (x, yy) = y.xy(cell);
                (board.sq_index[yy * board.width + x], phase as u16, parking, home as u16)
            })
            .collect(),
    )
}

/// Our order in the dump format of the instrumented YASS: one line per
/// entry, "x y phase parking home", phases ascending.
pub fn format(board: &Board, order: &[OrderEntry]) -> String {
    let mut s = String::new();
    for &(sq, phase, parking, home) in order {
        let cell = board.sq_pos[sq as usize];
        s.push_str(&format!("{} {} {} {} {}\n", cell % board.width, cell / board.width, phase, parking as u8, home));
    }
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Fallback: pull boxes off their goals, ignoring starting squares.
    PullBoxes,
    /// Pull boxes to unused starting squares, parking boxes when stuck.
    WithParking,
}

/// Player reachability with timestamps: `mark == stamp` is a reached
/// floor, `mark == stamp + 1` a box next to the reached area.
struct Reach {
    mark: Vec<u32>,
    stamp: u32,
    stack: Vec<usize>,
}

impl Reach {
    fn calc(&mut self, wall: &[bool], boxes: &[bool], off: &[isize; 4], player: usize) {
        self.stamp += 2;
        let st = self.stamp;
        self.mark[player] = st;
        self.stack.clear();
        self.stack.push(player);
        while let Some(c) = self.stack.pop() {
            for &o in off {
                let nb = (c as isize + o) as usize;
                if self.mark[nb] < st && !wall[nb] {
                    if boxes[nb] {
                        self.mark[nb] = st + 1;
                    } else {
                        self.mark[nb] = st;
                        self.stack.push(nb);
                    }
                }
            }
        }
    }
    fn floor(&self, c: usize) -> bool {
        self.mark[c] == self.stamp
    }
    /// A reached floor or a box touching the reached area.
    fn seen(&self, c: usize) -> bool {
        self.mark[c] >= self.stamp
    }
}

#[derive(Clone, Copy, Default)]
struct Space {
    sq: usize,
    player: usize,
}

/// Distances per (square, side of the player): pull[sq][d] is the number of
/// pulls bringing the box to sq with the player at sq + d.
type Dist = Vec<[u32; 4]>;

struct Yass {
    stride: usize,
    n: usize,
    off: [isize; 4],
    // Static board (after YASS's normalization).
    wall: Vec<bool>,
    goal: Vec<bool>,
    illegal: Vec<bool>,
    box_reach: Vec<bool>,
    gate: Vec<bool>,
    tunnel: Vec<bool>,
    /// Box starting squares as in the level text (before tube filling).
    start_flag: Vec<bool>,
    start_player: usize,
    /// Box starting squares, numbered in scan order.
    starts: Vec<usize>,
    box_reach_count: usize,
    goal_count: usize,
    /// target_dist[g][sq]: pulls from goal g to sq on the empty board.
    target_dist: Vec<Vec<u32>>,
    // Search state.
    boxes: Vec<bool>,
    player: usize,
    /// Goals, then parking squares (pseudo goals) in the order they appear.
    targets: Vec<usize>,
    /// For each target, the starting squares a box on it can be pulled to
    /// (empty for parking squares) and how many of them are still unused.
    edge_set: Vec<Vec<bool>>,
    edge_count: Vec<i64>,
    /// Phase assigned to each target (0: none yet).
    set_no: Vec<u32>,
    /// For a parking target, the phase of the goal its box came from.
    parked_from: Vec<u32>,
    /// Starting squares already claimed by a removed box.
    used: Vec<bool>,
    kind: Kind,
    park_countdown: u32,
    end_game: usize,
    large_area: usize,
    best_count: usize,
    best_set_no: Vec<u32>,
    best_remaining: usize,
    /// (phase count, target count) of the order found.
    result: (u32, usize),
    reach: Reach,
    deadline: Instant,
    timed_out: bool,
}

impl Yass {
    /// The padded grid with YASS's normalization and static square
    /// classification (`InitializeGame` up to the packing order).
    fn new(board: &Board, deadline: Instant) -> Option<Yass> {
        let stride = board.width + 2;
        let n = stride * (board.height + 2);
        let s = stride as isize;
        let off = [-s, -1, s, 1];
        let cell = |sq: u16| {
            let c = board.sq_pos[sq as usize];
            (c / board.width + 1) * stride + c % board.width + 1
        };
        let mut wall = vec![true; n];
        let mut goal = vec![false; n];
        let mut boxes = vec![false; n];
        for sq in 0..board.num_squares as u16 {
            wall[cell(sq)] = false;
        }
        for &g in &board.goals {
            goal[cell(g)] = true;
        }
        for &b in &board.start_boxes {
            boxes[cell(b)] = true;
        }
        let mut y = Yass {
            stride,
            n,
            off,
            wall,
            goal,
            illegal: vec![false; n],
            box_reach: vec![false; n],
            gate: vec![false; n],
            tunnel: vec![false; n],
            start_flag: vec![false; n],
            start_player: cell(board.start_player),
            starts: Vec::new(),
            box_reach_count: 0,
            goal_count: 0,
            target_dist: Vec::new(),
            boxes,
            player: cell(board.start_player),
            targets: Vec::new(),
            edge_set: Vec::new(),
            edge_count: Vec::new(),
            set_no: vec![0; MAX_TARGETS + 1],
            parked_from: vec![0; MAX_TARGETS + 1],
            used: Vec::new(),
            kind: Kind::WithParking,
            park_countdown: 0,
            end_game: 0,
            large_area: 0,
            best_count: 0,
            best_set_no: vec![0; MAX_TARGETS + 1],
            best_remaining: 0,
            result: (0, 0),
            reach: Reach { mark: vec![0; n], stamp: 0, stack: Vec::new() },
            deadline,
            timed_out: false,
        };
        y.remove_frozen_boxes()?;
        y.start_flag = y.boxes.clone();
        // Boxes are numbered in scan order before tube filling may push one.
        y.starts = (0..n).filter(|&c| y.boxes[c]).collect();
        y.fill_tubes();
        // Floors the player can never reach become walls.
        let mut no_boxes = vec![false; n];
        std::mem::swap(&mut no_boxes, &mut y.boxes);
        y.reach.calc(&y.wall, &y.boxes, &y.off, y.player);
        for c in 0..n {
            if !y.reach.floor(c) {
                y.wall[c] = true;
            }
        }
        std::mem::swap(&mut no_boxes, &mut y.boxes);
        let goals: Vec<usize> = (0..n).filter(|&c| y.goal[c] && !y.wall[c]).collect();
        if goals.len() != y.starts.len() || goals.len() > MAX_TARGETS {
            return None;
        }
        y.goal_count = goals.len();
        y.targets = goals;
        y.start_player = y.player;
        y.used = vec![false; y.starts.len()];

        // Static analysis with the boxes off the board.
        y.boxes = vec![false; n];
        y.mark_corners_and_closed_edges();
        y.calculate_box_reachable();
        y.calculate_gates();
        y.calculate_target_distances();
        y.mark_dead_squares();
        Some(y)
    }

    fn xy(&self, c: usize) -> (usize, usize) {
        (c % self.stride - 1, c / self.stride - 1)
    }

    fn manhattan(&self, a: usize, b: usize) -> i64 {
        let (ax, ay) = (a % self.stride, a / self.stride);
        let (bx, by) = (b % self.stride, b / self.stride);
        (ax.abs_diff(bx) + ay.abs_diff(by)) as i64
    }

    fn nb(&self, c: usize, d: usize) -> usize {
        (c as isize + self.off[d]) as usize
    }

    fn legal_reachable(&self, c: usize) -> bool {
        !self.wall[c] && !self.illegal[c] && self.box_reach[c]
    }

    fn terminated(&mut self) -> bool {
        if !self.timed_out && Instant::now() > self.deadline {
            self.timed_out = true;
        }
        self.timed_out
    }

    // ---- Board normalization ------------------------------------------

    /// Boxes frozen on goals at the start are walls (repeated, since a new
    /// wall can freeze more boxes or cut off squares). A frozen box off its
    /// goal makes the level unsolvable.
    fn remove_frozen_boxes(&mut self) -> Option<()> {
        loop {
            let no_boxes = vec![false; self.n];
            self.reach.calc(&self.wall, &no_boxes, &self.off, self.player);
            let reachable: Vec<bool> = (0..self.n).map(|c| self.reach.floor(c)).collect();
            let mut changed = false;
            for c in 0..self.n {
                if self.boxes[c] && (!reachable[c] || self.is_frozen(c)) {
                    if !self.goal[c] {
                        return None;
                    }
                    self.wall[c] = true;
                    self.goal[c] = false;
                    self.boxes[c] = false;
                    changed = true;
                }
            }
            for c in 0..self.n {
                if self.goal[c] && !reachable[c] && !self.wall[c] {
                    if !self.boxes[c] {
                        return None;
                    }
                    self.wall[c] = true;
                    self.goal[c] = false;
                    self.boxes[c] = false;
                    changed = true;
                }
            }
            if !changed {
                return Some(());
            }
        }
    }

    /// YASS's freeze test (`IsAFreezingMove` for a box already in place):
    /// blocked along both axes by walls or by boxes that are themselves
    /// blocked along the other axis (with this box counted as a wall).
    fn is_frozen(&mut self, c: usize) -> bool {
        let mut memo = vec![0u8; self.n];
        let mut non_goal = false;
        self.blocked_on_axis(c, UP, &mut memo, &mut non_goal) && self.blocked_on_axis(c, LEFT, &mut memo, &mut non_goal)
    }

    /// `d` names the axis to flip from (YASS passes the previous axis):
    /// UP checks the horizontal neighbors, anything else the vertical ones.
    /// Vertical results are memoized per call, as in YASS.
    fn blocked_on_axis(&mut self, c: usize, d: usize, memo: &mut Vec<u8>, non_goal: &mut bool) -> bool {
        let dir = if d == UP { LEFT } else { UP };
        let mut result;
        if dir == UP && memo[c] != 0 {
            result = memo[c] == 2;
        } else {
            let n1 = (c as isize - self.off[dir]) as usize;
            let n2 = self.nb(c, dir);
            let (w1, b1) = (self.wall[n1], self.boxes[n1]);
            let (w2, b2) = (self.wall[n2], self.boxes[n2]);
            self.wall[c] = true;
            result = w1 || w2 || (self.illegal[n1] && self.illegal[n2]);
            if !(result && *non_goal) && b1 && !w1 && self.blocked_on_axis(n1, dir, memo, non_goal) {
                result = true;
            }
            if !(result && *non_goal) && b2 && !w2 && self.blocked_on_axis(n2, dir, memo, non_goal) {
                result = true;
            }
            self.wall[c] = false;
        }
        if result && !self.goal[c] {
            *non_goal = true;
        }
        if dir == UP {
            memo[c] = 1 + result as u8;
        }
        result
    }

    /// Dead ends (floors with at most one floor neighbor, no goal, no box)
    /// become walls, repeatedly; a player standing in one steps out, pushing
    /// a box ahead if it must.
    fn fill_tubes(&mut self) {
        let box_count = self.boxes.iter().filter(|&&b| b).count();
        let mut on_goal = (0..self.n).filter(|&c| self.boxes[c] && self.goal[c]).count();
        loop {
            let mut more = false;
            for c in 0..self.n {
                if self.wall[c] || self.boxes[c] || self.goal[c] {
                    continue;
                }
                let floors: Vec<usize> = (0..4).filter(|&d| !self.wall[self.nb(c, d)]).collect();
                if floors.len() > 1 {
                    continue;
                }
                let mut dead_end = true;
                let mut step = None;
                if c == self.player {
                    match floors.last() {
                        None => dead_end = false,
                        Some(&d) => {
                            let np = self.nb(c, d);
                            let mut push = None;
                            if self.boxes[np] {
                                let nbx = self.nb(np, d);
                                dead_end = !self.wall[nbx] && !self.boxes[nbx] && on_goal < box_count;
                                push = Some(nbx);
                            }
                            step = Some((np, push));
                        }
                    }
                }
                if dead_end {
                    if let Some((np, push)) = step {
                        if let Some(nbx) = push {
                            on_goal -= (self.goal[np]) as usize;
                            self.boxes[np] = false;
                            let b = self.starts.iter().position(|&s| s == np).unwrap();
                            self.starts[b] = nbx;
                            self.boxes[nbx] = true;
                            on_goal += (self.goal[nbx]) as usize;
                        }
                        self.player = np;
                    }
                    self.wall[c] = true;
                    more = true;
                }
            }
            if !more {
                break;
            }
        }
    }

    // ---- Static square classification ---------------------------------

    /// Non-goal corners are illegal; so are the squares between two corners
    /// on a wall-lined line without goals (every square of the line has a
    /// wall on one side or the other, so a box on it can never leave it).
    fn mark_corners_and_closed_edges(&mut self) {
        let corners: Vec<usize> = (0..self.n)
            .filter(|&c| {
                !self.wall[c]
                    && (self.wall[self.nb(c, 0)] || self.wall[self.nb(c, 2)])
                    && (self.wall[self.nb(c, 1)] || self.wall[self.nb(c, 3)])
            })
            .collect();
        for &c in &corners {
            if !self.goal[c] {
                self.illegal[c] = true;
            }
        }
        for (i, &a) in corners.iter().enumerate() {
            for &b in &corners[i + 1..] {
                let ((ax, ay), (bx, by)) = (self.xy(a), self.xy(b));
                let (d, len) = if ay == by {
                    (3, bx - ax)
                } else if ax == bx {
                    (2, by - ay)
                } else {
                    continue;
                };
                let mut blocked = 0;
                let mut goals = 0;
                let mut c = a;
                for _ in 0..=len {
                    if self.wall[c] {
                        break;
                    }
                    goals += self.goal[c] as usize;
                    if self.wall[self.nb(c, left(d))] || self.wall[self.nb(c, right(d))] {
                        blocked += 1;
                    }
                    c = self.nb(c, d);
                }
                if blocked == len + 1 && goals == 0 {
                    let mut c = self.nb(a, d);
                    for _ in 1..len {
                        self.illegal[c] = true;
                        c = self.nb(c, d);
                    }
                }
            }
        }
    }

    /// Squares some box can be pushed to from its starting square, each box
    /// alone on the board.
    fn calculate_box_reachable(&mut self) {
        let mut dist: Dist = vec![[INF; 4]; self.n];
        let mut queue = Vec::new();
        for &s in &self.starts {
            self.boxes[s] = true;
            self.reach.calc(&self.wall, &self.boxes, &self.off, self.start_player);
            self.boxes[s] = false;
            for d in 0..4 {
                let nb = self.nb(s, d);
                if !self.wall[nb] && self.reach.floor(nb) {
                    queue.push((s, nb, 0));
                    dist[s][(d + 2) & 3] = 0;
                }
            }
        }
        let mut head = 0;
        let mut last_box = usize::MAX;
        while head < queue.len() {
            let (b, p, d0) = queue[head];
            head += 1;
            self.boxes[b] = true;
            if b != last_box || !self.reach.floor(p) {
                last_box = b;
                self.reach.calc(&self.wall, &self.boxes, &self.off, p);
            }
            for d in 0..4 {
                let to = self.nb(b, d);
                let from = (b as isize - self.off[d]) as usize;
                if !self.wall[to] && dist[to][d] > d0 + 1 && self.reach.floor(from) && !self.boxes[to] && !self.illegal[to] {
                    dist[to][d] = d0 + 1;
                    queue.push((to, b, d0 + 1));
                }
            }
            self.boxes[b] = false;
        }
        for c in 0..self.n {
            self.box_reach[c] = !self.wall[c] && dist[c].iter().any(|&x| x != INF);
        }
        for &s in &self.starts {
            self.box_reach[s] = true;
        }
        self.box_reach_count = self.box_reach.iter().filter(|&&r| r).count();
    }

    /// Gate squares: a box there splits the board into separate parts.
    fn calculate_gates(&mut self) {
        for c in 0..self.n {
            if !self.legal_reachable(c) || self.gate[c] {
                continue;
            }
            for d in 0..4 {
                let (nb, op) = (self.nb(c, d), (c as isize - self.off[d]) as usize);
                if self.wall[nb] || self.wall[op] {
                    continue;
                }
                let (l, r) = (self.nb(c, left(d)), self.nb(c, right(d)));
                if !(self.legal_reachable(l) || self.legal_reachable(r)) {
                    // No box can stand beside the square across this axis.
                    self.boxes[c] = true;
                    self.reach.calc(&self.wall, &self.boxes, &self.off, nb);
                    self.boxes[c] = false;
                    if !self.reach.floor(op) {
                        self.gate[c] = true;
                    }
                } else if (self.wall[l] && (self.wall[self.nb(nb, right(d))] || self.wall[self.nb(op, right(d))]))
                    || (self.wall[self.nb(nb, left(d))] && self.wall[self.nb(op, left(d))])
                {
                    self.boxes[c] = true;
                    self.reach.calc(&self.wall, &self.boxes, &self.off, nb);
                    if !self.reach.floor(op) {
                        if !self.wall[l] && !self.wall[r] {
                            self.reach.calc(&self.wall, &self.boxes, &self.off, l);
                            if !self.reach.floor(r) {
                                self.gate[c] = true;
                            }
                        } else {
                            self.gate[c] = true;
                        }
                    }
                    self.boxes[c] = false;
                }
            }
        }
    }

    /// Pull distances from every goal over the empty board, and for each
    /// goal the starting squares a box on it can be pulled to.
    fn calculate_target_distances(&mut self) {
        let k = self.starts.len();
        for gi in 0..self.goal_count {
            let g = self.targets[gi];
            let (dist, _) = self.pull_bfs(&[g], false);
            // YASS stores these as 16-bit values, the top one meaning none.
            let td: Vec<u32> = (0..self.n)
                .map(|c| match dist[c].iter().copied().min().unwrap() {
                    m if m != INF && self.legal_reachable(c) => m.min(u16::MAX as u32 - 1),
                    _ => INF,
                })
                .collect();
            let set: Vec<bool> = self.starts.iter().map(|&s| td[s] != INF).collect();
            self.edge_count.push(set.iter().filter(|&&b| b).count() as i64);
            self.edge_set.push(set);
            self.target_dist.push(td);
        }
        self.edge_set.resize(MAX_TARGETS + 1, vec![false; k]);
        self.edge_count.resize(MAX_TARGETS + 1, 0);
        self.targets.resize(MAX_TARGETS + 1, 0);
    }

    /// Floors no goal can be reached from (by pulls from the goals) are
    /// illegal.
    fn mark_dead_squares(&mut self) {
        let goals: Vec<usize> = self.targets[..self.goal_count].to_vec();
        let (dist, _) = self.pull_bfs(&goals, false);
        for c in 0..self.n {
            if !self.wall[c] && dist[c].iter().all(|&x| x == INF) {
                self.illegal[c] = true;
            }
        }
    }

    /// Goals in groups of connected goals: neighbors along one axis count
    /// as connected only if the goal has a wall or goal beside it on the
    /// other axis (a goal room rather than goals strung along a corridor).
    fn connected_goals(&self) -> usize {
        let mut group = vec![0usize; self.n];
        let mut total = 0;
        let mut groups = 0;
        for gi in 0..self.goal_count {
            let g = self.targets[gi];
            if group[g] != 0 {
                continue;
            }
            groups += 1;
            let count = self.find_connected(g, groups, &mut group);
            if count > 1 {
                total += count;
            }
        }
        total
    }

    fn find_connected(&self, g: usize, id: usize, group: &mut Vec<usize>) -> usize {
        let mut count = 1;
        group[g] = id;
        for d in 0..4 {
            let nb = self.nb(g, d);
            let side = |c: usize| self.wall[c] || self.goal[c];
            if self.goal[nb] && !self.wall[nb] && group[nb] == 0 && (side(self.nb(g, left(d))) || side(self.nb(g, right(d)))) {
                count += self.find_connected(nb, id, group);
            }
        }
        count
    }

    /// Forward tunnel squares ("do not stop here"): a box pushed there could
    /// as well be pushed on at once. Parking avoids them.
    fn calculate_tunnels(&mut self) {
        for c in 0..self.n {
            if !self.legal_reachable(c) {
                continue;
            }
            for d in 0..4 {
                let next = self.nb(c, d);
                if !self.legal_reachable(next) || self.goal[next] {
                    continue;
                }
                let walled = self.wall[self.nb(c, left(d))] && self.wall[self.nb(c, right(d))];
                let axis_blocked = {
                    let (a, b) = (self.nb(next, left(d)), self.nb(next, right(d)));
                    self.wall[a] || self.wall[b] || (self.illegal[a] && self.illegal[b])
                };
                if ((walled || self.gate[next]) && axis_blocked) || (walled && self.gate[c]) {
                    self.tunnel[next] = true;
                }
            }
        }
    }

    // ---- Box distance searches ----------------------------------------

    /// Breadth-first pull search from `sources` (other boxes stay put).
    /// With `use_player` the first pulls need the player's current reach;
    /// otherwise the player may start on any side. A pull may not leave the
    /// player in a dead end (walls on three sides) unless that square is
    /// the player's starting square. Returns the distances and the number of
    /// squares reached.
    fn pull_bfs(&mut self, sources: &[usize], use_player: bool) -> (Dist, usize) {
        let saved: Vec<bool> = sources.iter().map(|&s| self.boxes[s]).collect();
        for &s in sources {
            self.boxes[s] = false;
        }
        let mut dist: Dist = vec![[INF; 4]; self.n];
        let mut queue = Vec::new();
        if use_player {
            self.boxes[sources[0]] = true;
            self.reach.calc(&self.wall, &self.boxes, &self.off, self.player);
            self.boxes[sources[0]] = false;
        }
        for &s in sources {
            for d in 0..4 {
                let nb = self.nb(s, d);
                if !self.wall[nb] && (!use_player || self.reach.floor(nb)) {
                    queue.push((s, nb, 0));
                    dist[s][d] = 0;
                } else if !use_player {
                    dist[s][d] = 0;
                }
            }
        }
        let mut head = 0;
        let mut last_box = usize::MAX;
        while head < queue.len() {
            let (b, p, d0) = queue[head];
            head += 1;
            self.boxes[b] = true;
            if b != last_box || !self.reach.floor(p) {
                last_box = b;
                self.reach.calc(&self.wall, &self.boxes, &self.off, p);
            }
            for d in 0..4 {
                let to = self.nb(b, d);
                if self.wall[to] || dist[to][d] <= d0 + 1 || !self.reach.floor(to) || self.boxes[to] || self.illegal[to] {
                    continue;
                }
                let pto = self.nb(to, d);
                if self.wall[pto] || self.boxes[pto] {
                    continue;
                }
                let dead_end = self.wall[self.nb(pto, d)] && self.wall[self.nb(pto, left(d))] && self.wall[self.nb(pto, right(d))];
                if dead_end && pto != self.start_player {
                    continue;
                }
                dist[to][d] = d0 + 1;
                queue.push((to, pto, d0 + 1));
            }
            self.boxes[b] = false;
        }
        for (&s, &b) in sources.iter().zip(&saved) {
            self.boxes[s] = b;
        }
        let reached = (0..self.n).filter(|&c| !self.wall[c] && dist[c].iter().any(|&x| x != INF)).count();
        (dist, reached)
    }

    // ---- The packing-order search -------------------------------------

    /// `CalculatePackingOrder__`: try the search from the player's start,
    /// then from each other player region (boxes on all goals can split the
    /// board), in scan order.
    fn calculate(&mut self, kind: Kind) -> bool {
        self.kind = kind;
        self.set_no.iter_mut().for_each(|s| *s = 0);
        self.boxes.iter_mut().for_each(|b| *b = false);
        self.used.iter_mut().for_each(|u| *u = false);
        let k = self.starts.len();
        self.end_game = (k * (100 - END_GAME_PERCENT) + 50) / 100;
        self.large_area = self.box_reach_count * LARGE_AREA_PERCENT / 100;
        self.park_countdown = PARK_ATTEMPTS;
        self.best_remaining = k;
        let home = self.start_player;
        let mut visited = vec![false; self.n];
        let mut player = home;
        loop {
            for gi in 0..self.goal_count {
                self.boxes[self.targets[gi]] = true;
            }
            self.reach.calc(&self.wall, &self.boxes, &self.off, player);
            for c in 0..self.n {
                visited[c] |= self.reach.seen(c);
            }
            for gi in 0..self.goal_count {
                self.boxes[self.targets[gi]] = false;
            }
            if self.search_root(player) {
                return true;
            }
            let mut c = player;
            loop {
                c = if c + 1 < self.n { c + 1 } else { 0 };
                if (!visited[c] && !self.wall[c] && !self.goal[c]) || c == home {
                    break;
                }
            }
            player = c;
            if player == home || self.timed_out {
                return false;
            }
        }
    }

    /// `Search`: boxes on all goals, then peel. The fallback starts from
    /// the best partial order of the parking search (goals only).
    fn search_root(&mut self, player: usize) -> bool {
        self.player = player;
        for gi in 0..self.goal_count {
            self.boxes[self.targets[gi]] = true;
        }
        self.set_no.iter_mut().for_each(|s| *s = 0);
        self.parked_from.iter_mut().for_each(|s| *s = 0);
        let k = self.starts.len();
        let mut seen = vec![false; k];
        for (b, &s) in self.starts.iter().enumerate() {
            seen[b] = self.goal[s];
        }
        let (mut members, mut remaining, mut remaining_real) = (0, k, k);
        let mut phases = 0;
        if self.kind == Kind::PullBoxes && self.best_count > 0 && self.best_count <= MAX_TARGETS {
            phases = (0..self.goal_count).map(|g| self.best_set_no[g]).max().unwrap_or(0);
            for p in 1..=phases {
                for g in 0..self.goal_count {
                    if self.best_set_no[g] == p {
                        members += 1;
                        remaining -= 1;
                        remaining_real -= 1;
                        self.set_no[g] = p;
                        self.boxes[self.targets[g]] = false;
                    }
                }
            }
        }
        let found = self.search(phases + 1, members, self.goal_count, remaining, remaining_real, player, seen);
        for c in self.boxes.iter_mut() {
            *c = false;
        }
        self.player = player;
        found
    }

    /// `Search__`: one phase. Collects the boxes that can be removed now (or
    /// that should be parked), tries parking first when nothing can be
    /// removed or a box must be parked, then removes the collected boxes and
    /// recurses. `gap` is the number of targets (goals + parking squares).
    #[allow(clippy::too_many_arguments)]
    fn search(&mut self, set_count: u32, members: usize, gap: usize, remaining: usize, mut remaining_real: usize, player: usize, mut seen: Vec<bool>) -> bool {
        if remaining == 0 {
            self.result = (set_count, gap);
            return true;
        }
        if set_count > MAX_PHASES {
            return false;
        }
        self.player = player;
        self.reach.calc(&self.wall, &self.boxes, &self.off, player);
        let fr: Vec<u8> = (0..self.n).map(|c| if self.reach.floor(c) { 1 } else if self.reach.seen(c) { 2 } else { 0 }).collect();
        for (b, &s) in self.starts.iter().enumerate() {
            seen[b] |= fr[s] != 0;
        }

        // Candidates for removal (with the starting square each claims) and
        // for parking.
        let mut removal: Vec<(usize, Option<usize>)> = Vec::new();
        let mut parking: Vec<usize> = Vec::new();
        if seen.iter().all(|&s| s) || self.kind == Kind::PullBoxes {
            for g in 0..gap {
                let sq = self.targets[g];
                if self.set_no[g] != 0 || !self.boxes[sq] {
                    continue;
                }
                let pullable = (0..4).any(|d| {
                    let a = self.nb(sq, d);
                    !self.wall[a] && !self.boxes[a] && self.box_reach[a] && fr[self.nb(a, d)] == 1
                }) || (self.start_flag[sq] && self.all_boxes_at_starts(gap));
                if !pullable || self.terminated() {
                    continue;
                }
                let (start, area) = self.pull_to_start(g, remaining, members, gap);
                if start.is_none() && self.kind != Kind::PullBoxes {
                    continue;
                }
                if area >= self.large_area || remaining <= self.end_game || self.kind == Kind::PullBoxes || g >= self.goal_count {
                    // Boxes leaving goals and parked boxes moving on are
                    // never removed in the same phase; goals win.
                    let last_parked = removal.last().is_some_and(|&(h, _)| h >= self.goal_count);
                    if g < self.goal_count && last_parked {
                        self.clear_removal(&mut removal, &mut remaining_real);
                    } else if g >= self.goal_count && !removal.is_empty() && !last_parked {
                        continue;
                    }
                    self.add_removal(&mut removal, g, start, set_count, &mut remaining_real);
                } else {
                    parking.push(g);
                }
            }
        }
        // Parked boxes leave only after every real goal has been emptied.
        if removal.last().is_some_and(|&(h, _)| h >= self.goal_count) && remaining_real > 0 && parking.is_empty() {
            self.clear_removal(&mut removal, &mut remaining_real);
        }

        let mut found = false;
        if (!parking.is_empty() || (removal.is_empty() && self.kind == Kind::WithParking)) && gap < MAX_TARGETS && !self.terminated() {
            let pending = removal.clone();
            self.clear_removal(&mut removal, &mut remaining_real);
            // With parking candidates, only they may be parked.
            let mut tested = vec![!parking.is_empty(); gap];
            for &g in &parking {
                tested[g] = false;
            }
            while !found {
                let Some((g, spaces)) = self.find_parking(gap, &tested, &fr) else { break };
                tested[g] = true;
                self.set_no[g] = set_count;
                self.boxes[self.targets[g]] = false;
                remaining_real -= 1;
                for sp in spaces.iter().filter(|sp| sp.sq != 0) {
                    if found {
                        break;
                    }
                    self.boxes[sp.sq] = true;
                    self.targets[gap] = sp.sq;
                    self.set_no[gap] = 0;
                    self.parked_from[gap] = set_count;
                    self.edge_set[gap].iter_mut().for_each(|e| *e = false);
                    self.edge_count[gap] = 0;
                    found = self.search(set_count + 1, members + 1, gap + 1, remaining, remaining_real, sp.player, seen.clone());
                    self.boxes[sp.sq] = false;
                    if !found && members + 1 > self.best_count && self.best_remaining == self.starts.len() {
                        self.best_count = members + 1;
                        self.best_set_no = self.set_no.clone();
                    }
                }
                remaining_real += 1;
                if !found {
                    self.set_no[g] = 0;
                    self.parked_from[gap] = 0;
                }
                self.boxes[self.targets[g]] = true;
                self.player = player;
            }
            if !found {
                for (g, s) in pending {
                    self.add_removal(&mut removal, g, s, set_count, &mut remaining_real);
                }
            }
        }

        if !removal.is_empty() && !found {
            for &(g, _) in &removal {
                self.boxes[self.targets[g]] = false;
            }
            let m = members + removal.len();
            found = self.search(set_count + 1, m, gap, remaining - removal.len(), remaining_real, player, seen.clone());
            if !found && self.best_count <= MAX_TARGETS && (m > self.best_count || self.best_remaining == self.starts.len()) {
                self.best_count = m;
                self.best_set_no = self.set_no.clone();
                self.best_remaining = remaining - removal.len();
            }
            for &(g, _) in &removal {
                self.boxes[self.targets[g]] = true;
            }
            if !found {
                self.clear_removal(&mut removal, &mut remaining_real);
            }
        }
        found
    }

    fn all_boxes_at_starts(&self, gap: usize) -> bool {
        self.targets[..gap].iter().all(|&c| !self.boxes[c] || self.start_flag[c])
    }

    /// Tentatively assign target `g` to this phase, claiming its starting
    /// square.
    fn add_removal(&mut self, removal: &mut Vec<(usize, Option<usize>)>, g: usize, start: Option<usize>, set_count: u32, remaining_real: &mut usize) {
        removal.push((g, start));
        self.set_no[g] = set_count;
        if self.kind == Kind::WithParking {
            if let Some(b) = start {
                self.used[b] = true;
                for t in 0..self.goal_count {
                    if self.edge_set[t][b] {
                        self.edge_count[t] -= 1;
                    }
                }
            }
        }
        if g < self.goal_count {
            *remaining_real -= 1;
        }
    }

    fn clear_removal(&mut self, removal: &mut Vec<(usize, Option<usize>)>, remaining_real: &mut usize) {
        while let Some((g, start)) = removal.pop() {
            self.set_no[g] = 0;
            if self.kind == Kind::WithParking {
                if let Some(b) = start {
                    self.used[b] = false;
                    for t in 0..self.goal_count {
                        if self.edge_set[t][b] {
                            self.edge_count[t] += 1;
                        }
                    }
                }
            }
            if g < self.goal_count {
                *remaining_real += 1;
            }
        }
    }

    /// `CanPullBoxToStartingPosition`: the unused starting square the box
    /// on target `g` should be pulled to, if any, and the number of squares
    /// its pulls reach. Among the starting squares it can be pulled to
    /// (leaving the player able to walk back), the best one maximizes the
    /// least remaining choice of the open goals that could also use it.
    fn pull_to_start(&mut self, g: usize, remaining: usize, members: usize, gap: usize) -> (Option<usize>, usize) {
        let gsq = self.targets[g];
        let (dist, area) = self.pull_bfs(&[gsq], true);
        let player = self.player;
        self.boxes[gsq] = false;
        let mut best = None;
        let mut best_edges = -1i64;
        for b in 0..self.starts.len() {
            let s = self.starts[b];
            let near = self.manhattan(s, gsq) < MIN_REMOVAL_DISTANCE as i64;
            if self.used[b] || (near && !(s == gsq && g < self.goal_count) && remaining > self.end_game) {
                continue;
            }
            if s != gsq {
                for d in 0..4 {
                    if dist[s][d] == INF {
                        continue;
                    }
                    self.boxes[s] = true;
                    self.reach.calc(&self.wall, &self.boxes, &self.off, self.nb(s, d));
                    self.boxes[s] = false;
                    if self.reach.floor(player) || members + 1 >= gap {
                        let e = self.min_edges(b, gap);
                        if e > best_edges {
                            best = Some(b);
                            best_edges = e;
                        }
                        break;
                    }
                }
            } else {
                let e = self.min_edges(b, gap);
                if e > best_edges {
                    best = Some(b);
                    best_edges = e;
                }
            }
        }
        self.boxes[gsq] = true;
        self.player = player;
        (best, area)
    }

    /// The least count of unused starting squares among the open targets
    /// that can use starting square `b`.
    fn min_edges(&self, b: usize, gap: usize) -> i64 {
        (0..gap)
            .filter(|&t| self.set_no[t] == 0 && self.edge_set[t][b])
            .map(|t| self.edge_count[t])
            .min()
            .unwrap_or(i64::MAX)
    }

    /// `FindGoalCandidateForParking`: among the untested boxes on goals
    /// that the player can pull, the one whose parking square lies farthest
    /// from its goal (ties: the later one, unless it sits in a corner).
    fn find_parking(&mut self, gap: usize, tested: &[bool], fr: &[u8]) -> Option<(usize, [Space; 2])> {
        let mut best = None;
        let mut best_dist = -1i64;
        // YASS disables re-parking parked boxes, so only goals qualify.
        for g in 0..gap.min(self.goal_count) {
            let sq = self.targets[g];
            if self.set_no[g] != 0 || tested[g] || !self.boxes[sq] || self.park_countdown == 0 || self.terminated() {
                continue;
            }
            let corner = (0..4).any(|d| self.wall[self.nb(sq, d)] && self.wall[self.nb(sq, left(d))]);
            for d in 0..4 {
                let a = self.nb(sq, d);
                if !self.wall[a] && !self.boxes[a] && self.box_reach[a] && fr[self.nb(a, d)] == 1 {
                    if let Some((dist, spaces)) = self.parking_squares(g) {
                        if dist > best_dist || (dist == best_dist && !corner) {
                            best = Some((g, spaces));
                            best_dist = dist;
                        }
                    }
                    break;
                }
            }
        }
        if best.is_some() {
            self.park_countdown -= 1;
        }
        best
    }

    /// `CanPullBoxToParkingSquare`: up to two parking squares for the box on
    /// goal `g`, both reachable by pulls from where the box stands now and
    /// leaving the player able to walk back to the goal. The first maximizes
    /// the push distance back to the goal (on the empty board), the second
    /// the Manhattan distance from the goal (ties: farthest from the first).
    /// Gate and tunnel squares are avoided. Returns the first square's
    /// distance too.
    fn parking_squares(&mut self, g: usize) -> Option<(i64, [Space; 2])> {
        let box_sq = self.targets[g];
        let (pull, _) = self.pull_bfs(&[box_sq], true);
        self.boxes[box_sq] = false;
        let mut back = vec![i64::MAX; self.n];
        let (mut d1, mut d2) = (-1i64, -1i64);
        for c in 0..self.n {
            let m = pull[c].iter().copied().min().unwrap();
            if m == INF {
                continue;
            }
            let t = self.target_dist[g][c];
            if t != INF {
                back[c] = t as i64;
                d1 = d1.max(t as i64);
                d2 = d2.max(self.manhattan(box_sq, c));
            }
        }
        let mut spaces = [Space::default(); 2];
        let mut found = 0;
        loop {
            let mut first_found = false;
            'scan: for c in 1..self.n {
                if !self.box_reach[c] || self.illegal[c] || self.gate[c] || self.tunnel[c] {
                    continue;
                }
                let candidate = match found {
                    0 => back[c] == d1,
                    1 => self.manhattan(c, box_sq) == d2 && c != spaces[0].sq,
                    _ => self.manhattan(c, box_sq) == d2 && self.manhattan(c, spaces[0].sq) > self.manhattan(spaces[1].sq, spaces[0].sq),
                };
                if !candidate {
                    continue;
                }
                for d in 0..4 {
                    if pull[c][d] == INF {
                        continue;
                    }
                    let p = self.nb(c, d);
                    self.boxes[c] = true;
                    self.reach.calc(&self.wall, &self.boxes, &self.off, p);
                    self.boxes[c] = false;
                    if self.reach.floor(box_sq) {
                        found = (found + 1).min(2);
                        spaces[found - 1] = Space { sq: c, player: p };
                        if found == 1 {
                            first_found = true;
                            break 'scan;
                        }
                    }
                }
            }
            if !first_found {
                match found {
                    0 => d1 -= 1,
                    1 => d2 -= 1,
                    _ => {}
                }
            }
            if found == 2 || d1 <= 0 || d2 <= 0 {
                break;
            }
        }
        self.boxes[box_sq] = true;
        (found > 0).then_some((d1, spaces))
    }

    /// The order found: phases compacted (empty phases dropped), members of
    /// a phase in descending target number, as YASS lists them.
    fn order(&self) -> Vec<(usize, u32, bool, u32)> {
        let (mut count, gap) = self.result;
        let mut set_no = self.set_no[..gap].to_vec();
        let mut from = self.parked_from[..gap].to_vec();
        let mut size = vec![0usize; count as usize + 2];
        for &s in &set_no {
            size[s as usize] += 1;
        }
        let mut i = 1;
        while i <= count as usize {
            if size[i] != 0 {
                i += 1;
                continue;
            }
            for t in 0..gap {
                if set_no[t] as usize > i {
                    set_no[t] -= 1;
                }
                if from[t] as usize > i {
                    from[t] -= 1;
                }
            }
            count -= 1;
            size.remove(i);
        }
        let mut out = Vec::new();
        for p in 1..=count {
            for t in (0..gap).rev().filter(|&t| set_no[t] == p) {
                out.push((self.targets[t], p, t >= self.goal_count, from[t]));
            }
        }
        out
    }
}
