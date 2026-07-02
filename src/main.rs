mod bidir;
mod corral;
mod deadlock;
mod fess;
mod level;
mod matching;
mod solver;
#[cfg(test)]
mod tests;
mod verify;

use level::Board;
use solver::{Mode, Options, Outcome};
use std::time::Duration;

/// Racing portfolio (Sokolution-style): forward push-optimal A*, backward
/// pull search, weighted forward A*, and feature-space search (FESS-lite)
/// each run in their own thread with the FULL time budget; the first thread
/// to reach a definite answer (solution, or exhaustion of its complete
/// search space = unsolvability proof) stops the others. Returns the outcome
/// and which strategy produced it; forward-optimal wins ties so reported
/// solutions are push-optimal whenever that search finished in time.
fn solve_auto(board: &Board, base: &Options) -> (Outcome, &'static str) {
    use std::sync::atomic::Ordering;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mk = |mode: Mode| Options {
        mode,
        max_nodes: base.max_nodes,
        time_limit: base.time_limit,
        corral: base.corral,
        stop: stop.clone(),
    };
    let finishing = |outcome: Outcome, opts: &Options| -> Outcome {
        if !matches!(outcome, Outcome::Exhausted { .. }) {
            opts.stop.store(true, Ordering::Relaxed);
        }
        outcome
    };

    let (fwd, bwd, weighted, fess_out) = std::thread::scope(|s| {
        let o1 = mk(Mode::OptimalPushes);
        let o2 = mk(Mode::OptimalPushes);
        let mut o3a = mk(Mode::Weighted(3));
        o3a.time_limit = base.time_limit.mul_f64(0.5);
        let mut o3b = mk(Mode::Weighted(5));
        o3b.time_limit = base.time_limit.mul_f64(0.5);
        let o4 = mk(Mode::Greedy);
        let t1 = s.spawn(move || finishing(solver::solve(board, &o1), &o1));
        // Bidirectional meet-in-the-middle subsumes the plain backward
        // search (its backward half IS one) and adds forward meets. Like the
        // weighted slot, two orderings crack different levels: g+h for half
        // the budget, then g+2h.
        let mut o2a = o2.clone();
        o2a.time_limit = base.time_limit.mul_f64(0.5);
        let mut o2b = mk(Mode::Weighted(2));
        o2b.time_limit = base.time_limit.mul_f64(0.5);
        let t2 = s.spawn(move || match bidir::solve(board, &o2a) {
            Outcome::Exhausted { .. } => finishing(bidir::solve(board, &o2b), &o2b),
            other => finishing(other, &o2a),
        });
        // Weighted slot: w=3 for half the budget, then w=5 — different
        // weights crack different levels.
        let t3 = s.spawn(move || match solver::solve(board, &o3a) {
            Outcome::Exhausted { .. } => finishing(solver::solve(board, &o3b), &o3b),
            other => finishing(other, &o3a),
        });
        let t4 = s.spawn(move || finishing(fess::solve(board, &o4), &o4));
        (t1.join().unwrap(), t2.join().unwrap(), t3.join().unwrap(), t4.join().unwrap())
    });

    let results = [
        (fwd, "fwd-optimal"),
        (bwd, "bidir"),
        (weighted, "weighted"),
        (fess_out, "fess"),
    ];
    let mut solved = None;
    let mut unsolvable = None;
    let mut exhausted = None;
    for (outcome, tag) in results {
        match outcome {
            o @ Outcome::Solved { .. } => solved.get_or_insert((o, tag)),
            o @ Outcome::Unsolvable { .. } => unsolvable.get_or_insert((o, tag)),
            o @ Outcome::Exhausted { .. } => exhausted.get_or_insert((o, tag)),
        };
    }
    solved
        .or(unsolvable)
        .or(exhausted)
        .expect("at least one strategy result")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: sokoban-solver <levels.txt> [--level N] [--mode optimal|greedy|weighted:W] \
             [--time-limit SECS] [--max-nodes N] [--solutions FILE] [--quiet]"
        );
        std::process::exit(2);
    }

    let mut file = None;
    let mut only_level: Option<usize> = None;
    let mut opts = Options::default();
    let mut solutions_path: Option<String> = None;
    let mut quiet = false;
    let mut auto = true;
    let mut backward = false;
    let mut fess_mode = false;
    let mut bidir_mode = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--level" => only_level = Some(it.next().expect("--level N").parse().expect("level number")),
            "--mode" => {
                let m = it.next().expect("--mode MODE");
                auto = false;
                backward = false;
                opts.mode = match m.as_str() {
                    "auto" => {
                        auto = true;
                        Mode::OptimalPushes
                    }
                    "optimal" => Mode::OptimalPushes,
                    "greedy" => Mode::Greedy,
                    "backward" => {
                        backward = true;
                        Mode::OptimalPushes
                    }
                    "fess" => {
                        fess_mode = true;
                        Mode::Greedy
                    }
                    "bidir" => {
                        bidir_mode = true;
                        Mode::OptimalPushes
                    }
                    _ if m.starts_with("weighted:") => {
                        Mode::Weighted(m["weighted:".len()..].parse().expect("weight"))
                    }
                    _ => panic!("unknown mode {m}"),
                };
            }
            "--time-limit" => {
                opts.time_limit = Duration::from_secs_f64(
                    it.next().expect("--time-limit SECS").parse().expect("seconds"),
                )
            }
            "--max-nodes" => {
                opts.max_nodes = it.next().expect("--max-nodes N").parse().expect("node count")
            }
            "--solutions" => solutions_path = Some(it.next().expect("--solutions FILE").clone()),
            "--no-corral" => opts.corral = false,
            "--quiet" => quiet = true,
            _ if file.is_none() => file = Some(arg.clone()),
            _ => panic!("unexpected argument {arg}"),
        }
    }
    let file = file.expect("level file required");
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {file}: {e}"));
    let levels = level::parse_collection(&text);
    if levels.is_empty() {
        eprintln!("no levels found in {file}");
        std::process::exit(2);
    }

    let mut solved = 0usize;
    let mut attempted = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut total_time = Duration::ZERO;
    let mut total_expanded = 0u64;
    let mut solutions_out = String::new();

    for (i, lvl) in levels.iter().enumerate() {
        let n = i + 1;
        if let Some(only) = only_level {
            if n != only {
                continue;
            }
        }
        attempted += 1;
        let board = match level::Board::from_level(lvl) {
            Ok(b) => b,
            Err(e) => {
                println!("level {n} ({}): PARSE ERROR: {e}", lvl.name);
                failed.push(lvl.name.clone());
                continue;
            }
        };
        let (outcome, tag) = if auto {
            solve_auto(&board, &opts)
        } else if backward {
            (solver::solve_backward(&board, &opts), "backward")
        } else if fess_mode {
            (fess::solve(&board, &opts), "fess")
        } else if bidir_mode {
            (bidir::solve(&board, &opts), "bidir")
        } else {
            (solver::solve(&board, &opts), "single")
        };
        match outcome {
            Outcome::Solved { pushes, stats } => {
                let moves = verify::pushes_to_moves(&board, &pushes)
                    .unwrap_or_else(|| panic!("level {n}: reconstruction failed"));
                match verify::verify_lurd(lvl, &moves) {
                    Ok(push_count) => {
                        assert_eq!(push_count, pushes.len(), "level {n}: push count mismatch");
                        solved += 1;
                        total_time += stats.time;
                        total_expanded += stats.expanded;
                        if !quiet {
                            println!(
                                "level {n:>4} ({:>4}): solved  {:>4} pushes {:>5} moves  {:>9} nodes  {:>10.1?}  [{tag}]",
                                lvl.name,
                                pushes.len(),
                                moves.len(),
                                stats.expanded,
                                stats.time
                            );
                        }
                        solutions_out.push_str(&format!("; {}\n{}\n", lvl.name, moves));
                    }
                    Err(e) => {
                        println!("level {n} ({}): VERIFY FAILED: {e}", lvl.name);
                        failed.push(lvl.name.clone());
                    }
                }
            }
            Outcome::Unsolvable { stats } => {
                println!(
                    "level {n:>4} ({:>4}): UNSOLVABLE ({} nodes, {:.1?})",
                    lvl.name, stats.expanded, stats.time
                );
                failed.push(lvl.name.clone());
            }
            Outcome::Exhausted { stats } => {
                println!(
                    "level {n:>4} ({:>4}): TIMEOUT ({} nodes, {:.1?})",
                    lvl.name, stats.expanded, stats.time
                );
                failed.push(lvl.name.clone());
            }
        }
    }

    println!(
        "\n{solved}/{attempted} solved, total time {total_time:.2?}, total nodes {total_expanded}"
    );
    if !failed.is_empty() {
        println!("failed: {}", failed.join(", "));
    }
    if let Some(path) = solutions_path {
        std::fs::write(&path, solutions_out).expect("write solutions");
    }
    if solved != attempted {
        std::process::exit(1);
    }
}
