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

