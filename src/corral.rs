//! PI-corral pruning (Damgaard/Meger; used by YASS, JSoko, Sokolution,
//! Festival). "As important for Sokoban as alpha-beta is for 2-player games."
//!
//! A corral is a region of free squares the player cannot reach, fenced by
//! walls and boxes (the corral's boxes). If, even with every non-corral box
//! removed from the board, every possible push of a corral box is
//!   - inward (into this corral's free area), and
//!   - already executable in the real position right now,
//! then any solution can be reordered so that its first corral-touching push
//! comes first (pushing into the corral only grows player access, so the
//! postponed outside pushes stay valid). Provided the corral is "unfinished"
//! (a fence box off goal, or an empty goal inside when goals are scarce),
//! some corral push must eventually happen, so it is sound — and preserves
//! push-optimality — to generate ONLY those pushes at this node.
//!
//! The reduced (outside-boxes-removed) board over-approximates every future
//! first corral push: corral boxes cannot move before the first corral push
//! opens the fence, and removing outside boxes maximizes both player
//! reachability and free squares. A freeze deadlock found on the reduced
//! board only involves corral boxes and walls, and freezing is monotone in
//! added boxes, so excluding such pushes is sound too.
//!
//! Corollary: an unfinished corral with NO possible push on the reduced
//! board can never change — the node is a proven (corral) deadlock.

use crate::deadlock::FreezeChecker;
use crate::level::{Board, NONE};

const OPP: [usize; 4] = [1, 0, 3, 2];

pub struct CorralAnalyzer {
    /// Region id per free unreachable square this node, else 0. Stamped by
    /// (node_gen, region) pairs to avoid clearing: stores node_gen * shift + region.
    region_mark: Vec<u64>,
    node_gen: u64,
    /// Scratch: reduced-board box occupancy.
    reduced_box: Vec<bool>,
    /// Scratch: reduced-board player reachability, stamped per qualify() call.
    reduced_reach: Vec<u64>,
    reduced_gen: u64,
    /// Scratch queues / lists.
    queue: Vec<u16>,
    corral_boxes: Vec<u16>,
    region_squares: Vec<u16>,
    /// Corral mini-search: verdict cache keyed by (corral boxes, reduced
    /// player norm) — true = proven deadlock; false = not proven (also
    /// cached so budget isn't burned re-deriving unknowns).
    verdict_cache: rustc_hash::FxHashMap<(Box<[u16]>, u16), bool>,
    /// Remaining global node budget for mini-searches in this solve.
    mini_budget: u64,
}

pub enum CorralResult {
    /// No qualifying PI-corral: expand normally.
    NoPruning,
    /// Generate only these (box, dir) pushes.
    Restrict(Vec<(u16, u8)>),
    /// Proven corral deadlock: the node has no viable successors.
    Deadlock,
}

impl CorralAnalyzer {
    pub fn new(board: &Board) -> Self {
        CorralAnalyzer {
            region_mark: vec![0; board.num_squares],
            node_gen: 0,
            reduced_box: vec![false; board.num_squares],
            reduced_reach: vec![0; board.num_squares],
            reduced_gen: 0,
            queue: Vec::with_capacity(board.num_squares),
            corral_boxes: Vec::new(),
            region_squares: Vec::new(),
            verdict_cache: rustc_hash::FxHashMap::default(),
            mini_budget: 200_000,
        }
    }

    /// Analyze the current node. `box_at` is real box occupancy, `reach` the
    /// real player reachability test. `focus` is the square of the box moved
    /// by the last push: only corrals adjacent to it are examined (a push can
    /// only create corrals bordering the pushed box — splitting the player
    /// region happens across its square — so new corrals are never missed;
    /// pre-existing corrals were examined when they were created). Pass
    /// `None` (root node) to examine every corral.
    pub fn analyze(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: impl Fn(u16) -> bool,
        player: u16,
        freeze: &mut FreezeChecker,
        equal_goals_boxes: bool,
        focus: Option<u16>,
    ) -> CorralResult {
        self.node_gen += 1;
        let base = self.node_gen << 16;
        let mut next_region: u64 = 0;

        let mut best: Option<Vec<(u16, u8)>> = None;

        let starts: Vec<u16> = match focus {
            Some(sq) => board.neighbors[sq as usize]
                .iter()
                .filter(|&&n| n != NONE)
                .copied()
                .collect(),
            None => (0..board.num_squares as u16).collect(),
        };
        for start in starts {
            if box_at[start as usize]
                || reach(start)
                || self.region_mark[start as usize] >> 16 == self.node_gen
            {
                continue;
            }
            // New unreachable free region: flood it.
            next_region += 1;
            let region = base | next_region;
            self.region_squares.clear();
            self.corral_boxes.clear();
            self.queue.clear();
            self.queue.push(start);
            self.region_mark[start as usize] = region;
            let mut head = 0;
            while head < self.queue.len() {
                let sq = self.queue[head];
                head += 1;
                self.region_squares.push(sq);
                for d in 0..4 {
                    let n = board.neighbors[sq as usize][d];
                    if n == NONE {
                        continue;
                    }
                    if box_at[n as usize] {
                        // Fence box; collect once (mark with region on its square).
                        if self.region_mark[n as usize] != region {
                            self.region_mark[n as usize] = region;
                            self.corral_boxes.push(n);
                        }
                    } else if !reach(n) && self.region_mark[n as usize] != region {
                        self.region_mark[n as usize] = region;
                        self.queue.push(n);
                    }
                }
            }

            // Unfinished: a corral box off goal, or (when every goal must be
            // filled) an empty goal among the corral's free squares. Finished
            // corrals never justify pruning or deadlock verdicts.
            let unfinished = self
                .corral_boxes
                .iter()
                .any(|&b| !board.is_goal[b as usize])
                || (equal_goals_boxes
                    && self.region_squares.iter().any(|&s| board.is_goal[s as usize]));
            if !unfinished {
                continue;
            }

            match self.qualify(board, box_at, &reach, player, freeze, region) {
                Some(pushes) => {
                    if pushes.is_empty() {
                        return CorralResult::Deadlock;
                    }
                    if best.as_ref().is_none_or(|b| pushes.len() < b.len()) {
                        best = Some(pushes);
                    }
                }
                None => {
                    // Not a PI-corral: try to prove the corral dead outright
                    // with a small capped sub-search on the reduced board.
                    if self.mini_deadlock_search(board, player, freeze, equal_goals_boxes, region)
                    {
                        return CorralResult::Deadlock;
                    }
                }
            }
        }

        match best {
            Some(pushes) => CorralResult::Restrict(pushes),
            None => CorralResult::NoPruning,
        }
    }

    /// Check the PI-corral conditions for the region just flooded (marked with
    /// `region`; fence boxes in `self.corral_boxes`, free squares in
    /// `self.region_squares`). Returns the restricted push list if they hold.
    fn qualify(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: &impl Fn(u16) -> bool,
        player: u16,
        freeze: &mut FreezeChecker,
        region: u64,
    ) -> Option<Vec<(u16, u8)>> {
        // Reduced board: only corral boxes remain.
        for &b in &self.corral_boxes {
            self.reduced_box[b as usize] = true;
        }
        let result = self.qualify_inner(board, box_at, reach, player, freeze, region);
        for &b in &self.corral_boxes {
            self.reduced_box[b as usize] = false;
        }
        result
    }

    fn qualify_inner(
        &mut self,
        board: &Board,
        box_at: &[bool],
        reach: &impl Fn(u16) -> bool,
        player: u16,
        freeze: &mut FreezeChecker,
        region: u64,
    ) -> Option<Vec<(u16, u8)>> {
        // Player reachability on the reduced board (player square is free in
        // the reduced board too since it is free in the real one).
        self.reduced_gen += 1;
        let stamp = self.reduced_gen;
        self.queue.clear();
        self.queue.push(player);
        self.reduced_reach[player as usize] = stamp;
        let mut head = 0;
        while head < self.queue.len() {
            let sq = self.queue[head];
            head += 1;
            for d in 0..4 {
                let n = board.neighbors[sq as usize][d];
                if n != NONE
                    && self.reduced_reach[n as usize] != stamp
                    && !self.reduced_box[n as usize]
                {
                    self.reduced_reach[n as usize] = stamp;
                    self.queue.push(n);
                }
            }
        }

        // Enumerate every possible first push of a corral box on the reduced
        // board; all must be inward and executable right now for a PI-corral.
        let mut pushes = Vec::new();
        for i in 0..self.corral_boxes.len() {
            let c = self.corral_boxes[i];
            for d in 0..4 {
                let to = board.neighbors[c as usize][d];
                if to == NONE || self.reduced_box[to as usize] || board.dead[to as usize] {
                    continue;
                }
                let from = board.neighbors[c as usize][OPP[d]];
                if from == NONE
                    || self.reduced_box[from as usize]
                    || self.reduced_reach[from as usize] != stamp
                {
                    continue; // impossible while the corral is intact
                }
                // Freeze on the reduced board involves corral boxes only;
                // monotone, so safe to discard as "never a first push".
                self.reduced_box[c as usize] = false;
                self.reduced_box[to as usize] = true;
                let frozen = freeze.is_freeze_deadlock(board, &self.reduced_box, to);
                self.reduced_box[to as usize] = false;
                self.reduced_box[c as usize] = true;
                if frozen {
                    continue;
                }

                // Inward?
                if self.region_mark[to as usize] != region || box_at[to as usize] {
                    return None; // outward-capable fence box: not an I-corral
                }
                // Executable right now on the real board?
                if box_at[from as usize] || !reach(from) {
                    return None; // outside boxes currently block it: not PI
                }
                pushes.push((c, d as u8));
            }
        }
        Some(pushes)
    }
}

/// Corral deadlock mini-search (JSoko/Sokolution tier-5 detector).
///
/// On the reduced board (corral boxes only, all goals kept — removing goals
/// causes false positives), exhaustively search pushes of the corral boxes.
/// The reduced board over-approximates every real continuation restricted to
/// corral pushes: outside boxes only remove player access and free squares.
/// Real play could diverge from the model in exactly three ways, each of
/// which aborts with "not proven":
///   - a corral box is pushed outside the corral's closure (its free region
///     plus original fence squares),
///   - the player's region reaches into the corral's free area (the fence is
///     open; outside boxes may now interact),
///   - an outside box could be pushed INTO the corral (a free staging square
///     with a reachable pushing side lines up with a corral square) — boxes
///     can enter through an opened fence without the player ever entering.
/// If the capped search exhausts all states without any of those and without
/// resolving the corral (all corral boxes on goals, and no empty corral goal
/// when every goal must be filled), no continuation can ever resolve it:
/// the position is a proven deadlock. Verdicts are cached per corral.
impl CorralAnalyzer {
    fn mini_deadlock_search(
        &mut self,
        board: &Board,
        player: u16,
        freeze: &mut FreezeChecker,
        equal_goals_boxes: bool,
        region: u64,
    ) -> bool {
        const MAX_BOXES: usize = 10;
        const MAX_STATES: u64 = 400;
        if self.corral_boxes.len() > MAX_BOXES || self.mini_budget == 0 {
            return false;
        }

        // Reduced-board start norm (corral boxes only) keys the cache.
        for &b in &self.corral_boxes {
            self.reduced_box[b as usize] = true;
        }
        let start_norm = {
            self.reduced_gen += 1;
            let stamp = self.reduced_gen;
            self.queue.clear();
            self.queue.push(player);
            self.reduced_reach[player as usize] = stamp;
            let mut norm = player;
            let mut head = 0;
            while head < self.queue.len() {
                let sq = self.queue[head];
                head += 1;
                norm = norm.min(sq);
                for d in 0..4 {
                    let n = board.neighbors[sq as usize][d];
                    if n != NONE
                        && self.reduced_reach[n as usize] != stamp
                        && !self.reduced_box[n as usize]
                    {
                        self.reduced_reach[n as usize] = stamp;
                        self.queue.push(n);
                    }
                }
            }
            norm
        };
        for &b in &self.corral_boxes {
            self.reduced_box[b as usize] = false;
        }

        let mut start_boxes: Box<[u16]> = self.corral_boxes.clone().into_boxed_slice();
        start_boxes.sort_unstable();
        let cache_key = (start_boxes.clone(), start_norm);
        if let Some(&verdict) = self.verdict_cache.get(&cache_key) {
            return verdict;
        }

        let mut tt: rustc_hash::FxHashSet<(Box<[u16]>, u16)> = rustc_hash::FxHashSet::default();
        let mut stack: Vec<(Box<[u16]>, u16)> = vec![(start_boxes, player)];
        tt.insert(stack[0].clone());
        let mut states = 0u64;
        let mut proven = true; // falsified by any bail-out or budget stop

        'search: while let Some((boxes, ppos)) = stack.pop() {
            states += 1;
            if states > MAX_STATES || self.mini_budget == 0 {
                proven = false;
                break;
            }
            self.mini_budget -= 1;

            for &b in boxes.iter() {
                self.reduced_box[b as usize] = true;
            }
            // Player reachability on the mini board.
            self.reduced_gen += 1;
            let stamp = self.reduced_gen;
            self.queue.clear();
            self.queue.push(ppos);
            self.reduced_reach[ppos as usize] = stamp;
            let mut head = 0;
            while head < self.queue.len() {
                let sq = self.queue[head];
                head += 1;
                for d in 0..4 {
                    let n = board.neighbors[sq as usize][d];
                    if n != NONE
                        && self.reduced_reach[n as usize] != stamp
                        && !self.reduced_box[n as usize]
                    {
                        self.reduced_reach[n as usize] = stamp;
                        self.queue.push(n);
                    }
                }
            }

            // Bail: the player broke into the corral's free area.
            if self
                .region_squares
                .iter()
                .any(|&s| self.reduced_reach[s as usize] == stamp)
            {
                proven = false;
            }
            // Bail: an outside box could be staged and pushed into the corral.
            if proven {
                'ins: for &dest in &self.region_squares {
                    if self.reduced_box[dest as usize] {
                        continue;
                    }
                    for d in 0..4 {
                        let staging = board.neighbors[dest as usize][OPP[d]];
                        if staging == NONE || self.reduced_box[staging as usize] {
                            continue;
                        }
                        let pusher = board.neighbors[staging as usize][OPP[d]];
                        if pusher != NONE
                            && !self.reduced_box[pusher as usize]
                            && self.reduced_reach[pusher as usize] == stamp
                        {
                            proven = false;
                            break 'ins;
                        }
                    }
                }
            }
            // Resolved: corral finished — not a deadlock.
            if proven {
                let all_on_goals = boxes.iter().all(|&b| board.is_goal[b as usize]);
                let empty_goal = equal_goals_boxes
                    && self
                        .region_squares
                        .iter()
                        .any(|&s| board.is_goal[s as usize] && !self.reduced_box[s as usize]);
                if all_on_goals && !empty_goal {
                    proven = false;
                }
            }
            if !proven {
                for &b in boxes.iter() {
                    self.reduced_box[b as usize] = false;
                }
                break 'search;
            }

            // Expand pushes of the corral boxes.
            for bi in 0..boxes.len() {
                let b = boxes[bi];
                for d in 0..4 {
                    let to = board.neighbors[b as usize][d];
                    if to == NONE || self.reduced_box[to as usize] || board.dead[to as usize] {
                        continue;
                    }
                    let behind = board.neighbors[b as usize][OPP[d]];
                    if behind == NONE
                        || self.reduced_box[behind as usize]
                        || self.reduced_reach[behind as usize] != stamp
                    {
                        continue;
                    }
                    // Leaving the corral's closure: cannot prove anything.
                    if self.region_mark[to as usize] != region {
                        proven = false;
                        for &bb in boxes.iter() {
                            self.reduced_box[bb as usize] = false;
                        }
                        break 'search;
                    }
                    // Freeze on the mini board is monotone-sound: prune.
                    self.reduced_box[b as usize] = false;
                    self.reduced_box[to as usize] = true;
                    let frozen = freeze.is_freeze_deadlock(board, &self.reduced_box, to);
                    self.reduced_box[to as usize] = false;
                    self.reduced_box[b as usize] = true;
                    if frozen {
                        continue;
                    }
                    let mut child: Box<[u16]> = boxes.clone();
                    child[bi] = to;
                    child.sort_unstable();
                    let entry = (child, b);
                    if !tt.contains(&entry) {
                        tt.insert(entry.clone());
                        stack.push(entry);
                    }
                }
            }
            for &b in boxes.iter() {
                self.reduced_box[b as usize] = false;
            }
        }

        self.verdict_cache.insert(cache_key, proven);
        proven
    }
}
