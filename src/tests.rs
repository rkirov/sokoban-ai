//! End-to-end tests on small hand-written levels.

use crate::level::{parse_collection, Board};
use crate::solver::{solve, Mode, Options, Outcome};
use crate::verify::{pushes_to_moves, verify_lurd};

fn board(text: &str) -> (crate::level::Level, Board) {
    let levels = parse_collection(text);
    assert_eq!(levels.len(), 1, "expected exactly one level");
    let b = Board::from_level(&levels[0]).expect("parse");
    (levels[0].clone(), b)
}

/// Solve and fully verify; returns the push count.
fn solve_ok(text: &str, mode: Mode) -> usize {
    let (lvl, b) = board(text);
    let opts = Options { mode, ..Options::default() };
    match solve(&b, &opts) {
        Outcome::Solved { pushes, .. } => {
            let moves = pushes_to_moves(&b, &pushes).expect("reconstruction");
            let n = verify_lurd(&lvl, &moves).expect("replay");
            assert_eq!(n, pushes.len());
            pushes.len()
        }
        _ => panic!("expected solved"),
    }
}

fn unsolvable(text: &str) {
    let (_, b) = board(text);
    match solve(&b, &Options::default()) {
        Outcome::Unsolvable { .. } => {}
        Outcome::Solved { .. } => panic!("expected unsolvable, got a solution"),
        Outcome::Exhausted { .. } => panic!("expected unsolvable, hit limits"),
    }
}

#[test]
fn one_push() {
    assert_eq!(
        solve_ok(
            "#####\n\
             #@$.#\n\
             #####",
            Mode::OptimalPushes
        ),
        1
    );
}

#[test]
fn already_solved() {
    assert_eq!(
        solve_ok(
            "#####\n\
             #@* #\n\
             #####",
            Mode::OptimalPushes
        ),
        0
    );
}

#[test]
fn straight_line_optimal() {
    // Push one box three squares right.
    assert_eq!(
        solve_ok(
            "#######\n\
             #@$  .#\n\
             #######",
            Mode::OptimalPushes
        ),
        3
    );
}

#[test]
fn two_boxes() {
    // Two boxes in a row with open space above and below; the player must go
    // around to maneuver them onto the right-column goals.
    assert!(
        solve_ok(
            "########\n\
             #      #\n\
             #  @$$.#\n\
             #      #\n\
             #     .#\n\
             ########",
            Mode::OptimalPushes
        ) > 0
    );
}

#[test]
fn adjacent_boxes_against_wall_are_frozen() {
    // Both boxes hug the top wall and block each other: freeze deadlock from
    // the start, so the level is unsolvable.
    unsolvable(
        "########\n\
         #  @$$.#\n\
         #     .#\n\
         ########",
    );
}

#[test]
fn walk_around_needed() {
    // The player must walk around the box to push it left onto the goal.
    assert_eq!(
        solve_ok(
            "######\n\
             #    #\n\
             #.$@ #\n\
             #    #\n\
             ######",
            Mode::OptimalPushes
        ),
        1
    );
}

#[test]
fn corner_is_unsolvable() {
    // The only push jams the box into a corner; goal is elsewhere.
    unsolvable(
        "#####\n\
         #@$ #\n\
         ##. #\n\
         #####",
    );
}

#[test]
fn box_count_exceeds_goals_is_parse_error() {
    let levels = parse_collection(
        "#####\n\
         #@$$#\n\
         #.  #\n\
         #####",
    );
    assert!(Board::from_level(&levels[0]).is_err());
}

#[test]
fn freeze_deadlock_two_boxes_against_wall() {
    // Pushing either box up pins two boxes against the top wall off-goal:
    // the solver must find the alternate route or report unsolvable levels
    // correctly. This level IS solvable by pushing boxes sideways.
    assert!(
        solve_ok(
            "#######\n\
             #     #\n\
             # $$  #\n\
             # @ ..#\n\
             #######",
            Mode::OptimalPushes
        ) > 0
    );
}

#[test]
fn greedy_also_solves() {
    assert!(
        solve_ok(
            "########\n\
             #      #\n\
             #  @$$.#\n\
             #      #\n\
             #     .#\n\
             ########",
            Mode::Greedy
        ) > 0
    );
}

#[test]
fn microban_1_shape() {
    // Microban level 1 (known solvable).
    assert!(
        solve_ok(
            "####\n\
             # .#\n\
             #  ###\n\
             #*@  #\n\
             #  $ #\n\
             #  ###\n\
             ####",
            Mode::OptimalPushes
        ) > 0
    );
}

#[test]
fn tunnel_parking_is_required() {
    // Microban 1 level 10: a box must be PARKED inside the wall-flanked
    // row-4 corridor to vacate its square so the player can pass and handle
    // the other boxes first. A naive tunnel macro (force boxes through
    // wall-flanked runs) makes this level falsely unsolvable.
    assert!(
        solve_ok(
            "      #####\n\
             \u{20}     #.  #\n\
             \u{20}     #.# #\n\
             #######.# #\n\
             # @ $ $ $ #\n\
             # # # # ###\n\
             #       #\n\
             #########",
            Mode::OptimalPushes
        ) > 0
    );
}

#[test]
fn parse_collection_names_and_counts() {
    let text = "; 1\n\n####\n#@$.#\n####\n\n; 2\n\n#####\n#@$ .#\n#####\n";
    let levels = parse_collection(text);
    assert_eq!(levels.len(), 2);
    assert_eq!(levels[0].name, "1");
    assert_eq!(levels[1].name, "2");
}

#[test]
fn verify_rejects_walks_into_ragged_row_void() {
    // Row 1 is one cell short of the max width; the void cell must replay as
    // wall, not floor (found by adversarial review).
    let levels = parse_collection("######\n#. $@\n######");
    let lvl = &levels[0];
    assert!(crate::verify::verify_lurd(lvl, "rlLL").is_err());
    assert_eq!(crate::verify::verify_lurd(lvl, "LL"), Ok(2));
}


#[test]
fn backward_search_on_symmetric_board() {
    // Mirror-symmetric walls and goals, asymmetric start. The backward
    // search's target is the start position, which automorphisms do not
    // preserve, so it must not merge mirror-image states (it used to report
    // this one-push level UNSOLVABLE).
    let (lvl, b) = board(
        "#########\n\
         #@ $.   #\n\
         #########",
    );
    assert!(!b.automorphisms.is_empty(), "test needs a symmetric board");
    match crate::solver::solve_backward(&b, &Options::default()) {
        Outcome::Solved { pushes, .. } => {
            let moves = pushes_to_moves(&b, &pushes).expect("reconstruction");
            assert_eq!(verify_lurd(&lvl, &moves), Ok(1));
        }
        _ => panic!("expected the backward search to solve it"),
    }
}

/// Corral deadlock verdicts must be sound under the verdict cache: whenever
/// the analyzer, used the way the search uses it (warm cache, only corrals
/// adjacent to the last pushed box), reports a deadlock, a fresh analyzer
/// examining every corral must prove the position dead too. Positions come
/// from random push walks on real Microban levels. Keying the cache by
/// (corral boxes, player region) alone breaks this: one fence can enclose
/// several regions with different verdicts, so a verdict proven for one
/// region was reused for another (first unconfirmed claim: Microban III #7,
/// a position that happens to be dead anyway — the property is what this
/// test pins, independent of luck).
#[test]
fn corral_deadlock_verdicts_survive_the_cache() {
    use crate::corral::{CorralAnalyzer, CorralResult};
    use crate::deadlock::FreezeChecker;
    use crate::level::{NONE, OPP};

    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n as u64) as usize
    };

    let mut deadlocks = 0;
    for file in ["levels/microban2.txt", "levels/microban3.txt"] {
        let text = std::fs::read_to_string(file).unwrap();
        for (li, lvl) in parse_collection(&text).iter().enumerate() {
            let b = Board::from_level(lvl).unwrap();
            let equal = b.goals.len() == b.start_boxes.len();
            let mut warm = CorralAnalyzer::new(&b);
            let mut freeze = FreezeChecker::new(&b);
            let mut boxes = b.start_boxes.clone();
            let mut player = b.start_player;
            let mut focus = None;
            for step in 0..300 {
                if step % 30 == 0 {
                    boxes = b.start_boxes.clone();
                    player = b.start_player;
                    focus = None;
                }
                let mut box_at = vec![false; b.num_squares];
                for &x in &boxes {
                    box_at[x as usize] = true;
                }
                let mut reach = vec![false; b.num_squares];
                let mut stack = vec![player];
                reach[player as usize] = true;
                while let Some(s) = stack.pop() {
                    for &n in &b.neighbors[s as usize] {
                        if n != NONE && !reach[n as usize] && !box_at[n as usize] {
                            reach[n as usize] = true;
                            stack.push(n);
                        }
                    }
                }

                if warm.mini_budget() > 10_000 {
                    let w = warm.analyze(&b, &box_at, |s| reach[s as usize], player, &mut freeze, equal, focus);
                    if matches!(w, CorralResult::Deadlock) {
                        deadlocks += 1;
                        let mut cold = CorralAnalyzer::new(&b);
                        let c = cold.analyze(&b, &box_at, |s| reach[s as usize], player, &mut freeze, equal, None);
                        assert!(
                            matches!(c, CorralResult::Deadlock),
                            "{file} level {} step {step}: cached deadlock not confirmed",
                            li + 1
                        );
                    }
                }

                // Random legal push onto a live square.
                let mut pushes = Vec::new();
                for (i, &x) in boxes.iter().enumerate() {
                    for d in 0..4 {
                        let to = b.neighbors[x as usize][d];
                        let from = b.neighbors[x as usize][OPP[d]];
                        if to != NONE && from != NONE && !box_at[to as usize] && !b.dead[to as usize] && reach[from as usize] {
                            pushes.push((i, to, x));
                        }
                    }
                }
                if pushes.is_empty() {
                    boxes = b.start_boxes.clone();
                    player = b.start_player;
                    focus = None;
                    continue;
                }
                let (i, to, from) = pushes[next(pushes.len())];
                boxes[i] = to;
                player = from;
                focus = Some(to);
            }
        }
    }
    assert!(deadlocks > 100, "only {deadlocks} deadlock verdicts exercised");
}

#[test]
fn frozen_walls_seal_goals_behind_them() {
    use crate::deadlock::FrozenWalls;
    use crate::level::INF;
    // One-wide corridor: goals at x=1 and x=2, box start x=5.
    let (_, b) = board("########\n#.*  $@#\n########");
    let sq = |x: usize| b.sq_index[b.width + x];
    let mut fw = FrozenWalls::new(&b);
    let open = fw.distances(&b, &[]);
    assert_eq!(*open, b.goal_dist, "no walls = the board's own tables");
    let deep = b.goals.iter().position(|&g| g == sq(1)).unwrap();
    assert_ne!(open[deep][sq(5) as usize], INF);
    let walled = fw.distances(&b, &[sq(2)]);
    assert_eq!(walled[deep][sq(5) as usize], INF, "a frozen box on x=2 seals x=1");
}

#[test]
fn gate_pushes_are_forced_only_through_articulation_squares() {
    // A one-wide passage between two rooms: pushing a box from the left
    // room into the passage entrance (a gate square) forces it onward.
    let (_, b) = board(
        "#########\n\
         #   #   #\n\
         # @$  . #\n\
         #   #   #\n\
         #########",
    );
    let sq = |x: usize, y: usize| b.sq_index[y * b.width + x] as usize;
    let right = 3; // DIRS index for +x
    assert!(b.forced[sq(3, 2)][right], "(3,2)->(4,2): (4,2) is the only link between the rooms");
    // Inside an open room nothing is forced.
    assert!(!b.forced[sq(1, 2)][right]);
}
