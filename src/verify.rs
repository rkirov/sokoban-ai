//! Solution reconstruction and independent verification.
//!
//! `pushes_to_moves` expands a push sequence into a full LURD move string by
//! BFS-ing the player's walk between pushes. `verify_lurd` then replays that
//! string against the *original parsed level* (not the solver's normalized
//! board) as an independent correctness check.

use crate::level::{Board, Level, DIRS, DIR_CHARS, NONE};

const OPP: [usize; 4] = [1, 0, 3, 2];

/// Expand pushes into a LURD string (lowercase = walk, uppercase = push).
pub fn pushes_to_moves(board: &Board, pushes: &[(u16, u8)]) -> Option<String> {
    let mut box_at = vec![false; board.num_squares];
    for &b in &board.start_boxes {
        box_at[b as usize] = true;
    }
    let mut player = board.start_player;
    let mut moves = String::new();

    let mut prev = vec![(NONE, 0u8); board.num_squares];
    let mut visited = vec![0u32; board.num_squares];
    let mut generation = 0u32;

    for &(box_from, dir) in pushes {
        let dir = dir as usize;
        if !box_at[box_from as usize] {
            return None;
        }
        let stand = board.neighbors[box_from as usize][OPP[dir]];
        let to = board.neighbors[box_from as usize][dir];
        if stand == NONE || to == NONE || box_at[to as usize] || box_at[stand as usize] {
            return None;
        }

        // BFS walk from `player` to `stand`.
        if player != stand {
            generation += 1;
            let mut queue = vec![player];
            visited[player as usize] = generation;
            let mut head = 0;
            let mut found = false;
            while head < queue.len() {
                let sq = queue[head];
                head += 1;
                if sq == stand {
                    found = true;
                    break;
                }
                for d in 0..4 {
                    let n = board.neighbors[sq as usize][d];
                    if n != NONE && visited[n as usize] != generation && !box_at[n as usize] {
                        visited[n as usize] = generation;
                        prev[n as usize] = (sq, d as u8);
                        queue.push(n);
                    }
                }
            }
            if !found {
                return None;
            }
            let mut path = Vec::new();
            let mut cur = stand;
            while cur != player {
                let (p, d) = prev[cur as usize];
                path.push(DIR_CHARS[d as usize]);
                cur = p;
            }
            path.reverse();
            moves.extend(path);
        }

        moves.push(DIR_CHARS[dir].to_ascii_uppercase());
        box_at[box_from as usize] = false;
        box_at[to as usize] = true;
        player = box_from;
    }

    if box_at
        .iter()
        .enumerate()
        .any(|(sq, &occupied)| occupied && !board.is_goal[sq])
    {
        return None;
    }
    Some(moves)
}

/// Replay a LURD string against the raw level text. Returns the number of
/// pushes if the replay is legal and ends with every box on a goal.
pub fn verify_lurd(level: &Level, moves: &str) -> Result<usize, String> {
    let height = level.rows.len();
    let width = level.rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut wall = vec![false; width * height];
    let mut goal = vec![false; width * height];
    let mut boxes = vec![false; width * height];
    let mut player = usize::MAX;
    for (y, row) in level.rows.iter().enumerate() {
        // Pad short rows with wall, mirroring Board::from_level, so the
        // ragged-row void is not replayable floor.
        for x in row.len()..width {
            wall[y * width + x] = true;
        }
        for (x, &c) in row.iter().enumerate() {
            let cell = y * width + x;
            match c {
                b'#' => wall[cell] = true,
                b'.' => goal[cell] = true,
                b'$' => boxes[cell] = true,
                b'*' => {
                    boxes[cell] = true;
                    goal[cell] = true;
                }
                b'@' => player = cell,
                b'+' => {
                    player = cell;
                    goal[cell] = true;
                }
                _ => {}
            }
        }
    }
    if player == usize::MAX {
        return Err("no player".into());
    }

    let mut pushes = 0usize;
    for (i, mv) in moves.chars().enumerate() {
        let d = match mv.to_ascii_lowercase() {
            'u' => 0,
            'd' => 1,
            'l' => 2,
            'r' => 3,
            _ => return Err(format!("bad move char {mv:?} at {i}")),
        };
        let (dx, dy) = DIRS[d];
        let step = |cell: usize| -> Option<usize> {
            let (x, y) = ((cell % width) as i32 + dx, (cell / width) as i32 + dy);
            (x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height)
                .then(|| y as usize * width + x as usize)
        };
        let next = step(player).ok_or_else(|| format!("move {i} leaves the board"))?;
        if wall[next] {
            return Err(format!("move {i} walks into a wall"));
        }
        if boxes[next] {
            if !mv.is_ascii_uppercase() {
                return Err(format!("move {i} is a push written as a walk"));
            }
            let beyond = step(next).ok_or_else(|| format!("push {i} leaves the board"))?;
            if wall[beyond] || boxes[beyond] {
                return Err(format!("push {i} is blocked"));
            }
            boxes[next] = false;
            boxes[beyond] = true;
            pushes += 1;
        } else if mv.is_ascii_uppercase() {
            return Err(format!("move {i} is a walk written as a push"));
        }
        player = next;
    }

    for cell in 0..width * height {
        if boxes[cell] && !goal[cell] {
            return Err("a box is not on a goal after replay".into());
        }
    }
    Ok(pushes)
}
