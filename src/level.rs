//! Level parsing and static board representation.
//!
//! A parsed level is normalized into a `Board`: the set of squares the player
//! can ever reach (everything else becomes wall), with per-square neighbor
//! tables, simple-deadlock ("dead") squares, and per-goal pull distances used
//! as an admissible push-distance lower bound.

pub const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)]; // U, D, L, R
pub const DIR_CHARS: [char; 4] = ['u', 'd', 'l', 'r'];
pub const OPP: [usize; 4] = [1, 0, 3, 2];

pub const NONE: u16 = u16::MAX;
pub const INF: u32 = u32::MAX;

#[derive(Clone)]
pub struct Level {
    pub name: String,
    pub rows: Vec<Vec<u8>>,
}

/// Static, precomputed data for one level. Some precomputed tables (tunnel
/// flags, per-square min goal distance, grid geometry) are not consumed by
/// the current search but are kept for planned techniques (macro moves,
/// packing plans) and debugging.
#[allow(dead_code)]
pub struct Board {
    pub name: String,
    pub width: usize,
    pub height: usize,
    /// Grid cell -> square index, or NONE for walls / unreachable cells.
    pub sq_index: Vec<u16>,
    /// Square index -> grid cell.
    pub sq_pos: Vec<usize>,
    pub num_squares: usize,
    /// neighbors[sq][dir] = adjacent square in dir, or NONE if wall.
    pub neighbors: Vec<[u16; 4]>,
    pub goals: Vec<u16>,
    pub is_goal: Vec<bool>,
    /// Simple-deadlock squares: a box here can never reach any goal.
    pub dead: Vec<bool>,
    /// goal_dist[g][sq]: relaxed push distance from sq to goals[g] (INF if unreachable).
    pub goal_dist: Vec<Vec<u32>>,
    /// min_goal_dist[sq]: min over goals of goal_dist (INF on dead squares).
    pub min_goal_dist: Vec<u32>,
    /// start_dist[k][sq]: relaxed push distance from start_boxes[k] to sq —
    /// equivalently the pull distance from sq back to that start square.
    /// Used by the backward (pull) search as its admissible lower bound.
    pub start_dist: Vec<Vec<u32>>,
    /// Squares from which a box can never be pulled back to any start square:
    /// dead for the backward search.
    pub backward_dead: Vec<bool>,
    /// tunnel[sq][dir]: pushing a box into `sq` moving in `dir` forces it onward:
    /// sq is not a goal and the two squares orthogonal to `dir` are walls.
    pub tunnel: Vec<[bool; 4]>,
    pub start_boxes: Vec<u16>,
    pub start_player: u16,
    /// Non-identity board automorphisms: square-index permutations from the
    /// dihedral transforms of the grid that map walls to walls and goals to
    /// goals. States equivalent under one of these have identical
    /// solvability and solution length, so transposition keys can be
    /// canonicalized over the group (arena nodes keep actual states, so
    /// reconstruction is unaffected).
    pub automorphisms: Vec<Vec<u16>>,
}

/// Split a level-collection file into individual levels.
///
/// Board lines consist of the standard characters `# @+$*.` (with `-`/`_`
/// accepted as floor) and contain at least one `#`. Lines starting with `;`
/// name the following level. Consecutive board lines form one level.
pub fn parse_collection(text: &str) -> Vec<Level> {
    let mut levels = Vec::new();
    let mut pending_name: Option<String> = None;
    let mut current: Vec<Vec<u8>> = Vec::new();

    let flush = |current: &mut Vec<Vec<u8>>, pending_name: &mut Option<String>, levels: &mut Vec<Level>| {
        if !current.is_empty() {
            let name = pending_name
                .take()
                .unwrap_or_else(|| format!("{}", levels.len() + 1));
            levels.push(Level { name, rows: std::mem::take(current) });
        }
    };

    for line in text.lines() {
        let trimmed = line.trim_end();
        let is_board_line = !trimmed.is_empty()
            && trimmed.contains('#')
            && trimmed.chars().all(|c| "# @+$*.-_".contains(c));
        if is_board_line {
            let row = trimmed
                .bytes()
                .map(|b| if b == b'-' || b == b'_' { b' ' } else { b })
                .collect();
            current.push(row);
        } else {
            flush(&mut current, &mut pending_name, &mut levels);
            if let Some(rest) = trimmed.strip_prefix(';') {
                let rest = rest.trim();
                if !rest.is_empty() {
                    pending_name = Some(rest.to_string());
                }
            }
        }
    }
    flush(&mut current, &mut pending_name, &mut levels);
    levels
}

impl Board {
    /// Canonical transposition key over the automorphism group: the
    /// lexicographic minimum of (mapped sorted boxes, mapped player-region
    /// norm). `region` holds the squares of the player's reachable region.
    /// With no automorphisms this is just (boxes, norm).
    pub fn canonical_key(&self, boxes: &[u16], region: &[u16], norm: u16) -> (Box<[u16]>, u16) {
        let mut best: (Box<[u16]>, u16) = (boxes.into(), norm);
        for perm in &self.automorphisms {
            let mut mb: Vec<u16> = boxes.iter().map(|&b| perm[b as usize]).collect();
            mb.sort_unstable();
            let mn = region.iter().map(|&s| perm[s as usize]).min().unwrap_or(norm);
            if (mb.as_slice(), mn) < (best.0.as_ref(), best.1) {
                best = (mb.into_boxed_slice(), mn);
            }
        }
        best
    }

    pub fn from_level(level: &Level) -> Result<Board, String> {
        let height = level.rows.len();
        let width = level.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if height == 0 || width == 0 {
            return Err("empty level".into());
        }
        let mut grid = vec![b'#'; width * height]; // pad short rows with wall
        for (y, row) in level.rows.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                grid[y * width + x] = c;
            }
        }

        let mut player_cell = None;
        for (i, &c) in grid.iter().enumerate() {
            if c == b'@' || c == b'+' {
                if player_cell.replace(i).is_some() {
                    return Err("multiple players".into());
                }
            }
        }
        let player_cell = player_cell.ok_or("no player")?;

        // Flood fill from the player through non-wall cells; everything the
        // player can never reach is treated as wall.
        let mut reachable = vec![false; width * height];
        let mut stack = vec![player_cell];
        reachable[player_cell] = true;
        while let Some(cell) = stack.pop() {
            let (x, y) = ((cell % width) as i32, (cell / width) as i32);
            for (dx, dy) in DIRS {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                let ncell = ny as usize * width + nx as usize;
                if !reachable[ncell] && grid[ncell] != b'#' {
                    reachable[ncell] = true;
                    stack.push(ncell);
                }
            }
        }

        // Index reachable squares.
        let mut sq_index = vec![NONE; width * height];
        let mut sq_pos = Vec::new();
        for cell in 0..width * height {
            if reachable[cell] {
                sq_index[cell] = sq_pos.len() as u16;
                sq_pos.push(cell);
            } else if grid[cell] == b'$' {
                // A box the player can never touch, not on a goal: unsolvable.
                return Err("box off goal outside reachable area".into());
            }
            // An unreachable '*' is a decorative box permanently parked on
            // its goal; both it and the goal drop out of the puzzle.
        }
        let num_squares = sq_pos.len();
        if num_squares > NONE as usize {
            return Err("level too large".into());
        }

        let mut neighbors = vec![[NONE; 4]; num_squares];
        for (sq, &cell) in sq_pos.iter().enumerate() {
            let (x, y) = ((cell % width) as i32, (cell / width) as i32);
            for (d, (dx, dy)) in DIRS.iter().enumerate() {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                neighbors[sq][d] = sq_index[ny as usize * width + nx as usize];
            }
        }

        let mut goals = Vec::new();
        let mut is_goal = vec![false; num_squares];
        let mut start_boxes = Vec::new();
        for (sq, &cell) in sq_pos.iter().enumerate() {
            match grid[cell] {
                b'.' | b'+' | b'*' => {
                    is_goal[sq] = true;
                    goals.push(sq as u16);
                }
                _ => {}
            }
            if matches!(grid[cell], b'$' | b'*') {
                start_boxes.push(sq as u16);
            }
        }
        start_boxes.sort_unstable();
        if start_boxes.len() > goals.len() {
            return Err(format!(
                "{} boxes but only {} goals",
                start_boxes.len(),
                goals.len()
            ));
        }
        if start_boxes.is_empty() {
            return Err("no boxes".into());
        }

        // Per-goal pull BFS. A box at `q` can be pulled to `n` (adjacent in
        // dir d) if both `n` and the square beyond `n` are floor; the reverse
        // of this relaxed pull is a relaxed push, so dist[sq] is an admissible
        // lower bound on pushes needed to bring a box from sq to the goal.
        let mut goal_dist = Vec::with_capacity(goals.len());
        for &g in &goals {
            let mut dist = vec![INF; num_squares];
            dist[g as usize] = 0;
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(g);
            while let Some(q) = queue.pop_front() {
                for d in 0..4 {
                    let n = neighbors[q as usize][d];
                    if n == NONE {
                        continue;
                    }
                    let beyond = neighbors[n as usize][d];
                    if beyond == NONE {
                        continue;
                    }
                    if dist[n as usize] == INF {
                        dist[n as usize] = dist[q as usize] + 1;
                        queue.push_back(n);
                    }
                }
            }
            goal_dist.push(dist);
        }

        let mut min_goal_dist = vec![INF; num_squares];
        for dist in &goal_dist {
            for sq in 0..num_squares {
                min_goal_dist[sq] = min_goal_dist[sq].min(dist[sq]);
            }
        }
        let dead: Vec<bool> = (0..num_squares).map(|sq| min_goal_dist[sq] == INF).collect();

        // Per-start-box relaxed push BFS: pushing a box from q to q+d needs
        // q+d (destination) and q-d (player) free. start_dist[k][s] is then
        // both "push distance start->s" and "pull distance s->start", the
        // backward search's admissible lower bound.
        let mut start_dist = Vec::with_capacity(start_boxes.len());
        for &s0 in &start_boxes {
            let mut dist = vec![INF; num_squares];
            dist[s0 as usize] = 0;
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(s0);
            while let Some(q) = queue.pop_front() {
                for d in 0..4 {
                    let dest = neighbors[q as usize][d];
                    if dest == NONE {
                        continue;
                    }
                    let player = neighbors[q as usize][OPP[d]];
                    if player == NONE {
                        continue;
                    }
                    if dist[dest as usize] == INF {
                        dist[dest as usize] = dist[q as usize] + 1;
                        queue.push_back(dest);
                    }
                }
            }
            start_dist.push(dist);
        }
        let backward_dead: Vec<bool> = (0..num_squares)
            .map(|sq| start_dist.iter().all(|d| d[sq] == INF))
            .collect();

        // Tunnel detection: entering `sq` moving in `dir`, with walls on both
        // orthogonal sides and no goal here, the box must keep moving.
        let mut tunnel = vec![[false; 4]; num_squares];
        for sq in 0..num_squares {
            if is_goal[sq] {
                continue;
            }
            for d in 0..4 {
                let (o1, o2) = if d < 2 { (2, 3) } else { (0, 1) };
                if neighbors[sq][o1] == NONE && neighbors[sq][o2] == NONE {
                    tunnel[sq][d] = true;
                }
            }
        }

        // Board automorphisms: dihedral transforms of the bounding rectangle
        // under which the playable-square set and the goal set are invariant.
        // (Boxes and player need not be symmetric — only static geometry.)
        let mut automorphisms = Vec::new();
        let (w, h) = (width as i32, height as i32);
        let transforms: [(bool, Box<dyn Fn(i32, i32) -> (i32, i32)>); 7] = [
            (true, Box::new(move |x, y| (w - 1 - x, y))),          // mirror x
            (true, Box::new(move |x, y| (x, h - 1 - y))),          // mirror y
            (true, Box::new(move |x, y| (w - 1 - x, h - 1 - y))),  // rotate 180
            (w == h, Box::new(move |x, y| (y, x))),                // transpose
            (w == h, Box::new(move |x, y| (h - 1 - y, w - 1 - x))), // anti-transpose
            (w == h, Box::new(move |x, y| (h - 1 - y, x))),        // rotate 90
            (w == h, Box::new(move |x, y| (y, w - 1 - x))),        // rotate 270
        ];
        for (applicable, t) in &transforms {
            if !applicable {
                continue;
            }
            let mut perm = vec![NONE; num_squares];
            let mut ok = true;
            for sq in 0..num_squares {
                let cell = sq_pos[sq];
                let (x, y) = ((cell % width) as i32, (cell / width) as i32);
                let (nx, ny) = t(x, y);
                let ncell = ny as usize * width + nx as usize;
                let nsq = sq_index[ncell];
                if nsq == NONE || is_goal[sq] != is_goal[nsq as usize] {
                    ok = false;
                    break;
                }
                perm[sq] = nsq;
            }
            if ok {
                automorphisms.push(perm);
            }
        }

        let start_player = sq_index[player_cell];
        Ok(Board {
            name: level.name.clone(),
            width,
            height,
            sq_index,
            sq_pos,
            num_squares,
            neighbors,
            goals,
            is_goal,
            dead,
            goal_dist,
            min_goal_dist,
            start_dist,
            backward_dead,
            tunnel,
            start_boxes,
            start_player,
            automorphisms,
        })
    }
}
