mod corral;
mod deadlock;
mod deadsets;
mod fess;
mod hotspots;
mod level;
mod macros;
mod matching;
mod packing;
mod retro;
mod solver;
#[cfg(test)]
mod tests;
mod verify;

use level::Board;
use solver::{Mode, Options, Outcome};
use std::time::Duration;

/// One thread of the racing portfolio. The default portfolio was chosen by
/// measuring each strategy's unique solves at 10 s/level: optimal A* (also
/// gives push-optimal answers), FESS (large levels), and backward search
/// twice — optimal and greedy (f = h) — since cramped goal areas are easy
/// backward and the two orderings solve different levels (greedy replaced
/// weighted w = 3: SokHard +9, Sasquatch +3, Microban -2). (Bidirectional
/// search and staged weighted forward A* were measured and removed: they
/// added no solves; see EXPERIMENTS.md.)
#[derive(Clone, Copy)]
enum Strategy {
    /// Forward push-optimal A*.
    Optimal,
    /// Backward (pull) search from the goal with f = g + w*h (w = 1:
    /// optimal; w = 0: greedy, f = h): cramped goal areas are easy backward.
    Backward(u32),
    /// Feature-space search over macro moves.
    Fess,
}

impl Strategy {
    const DEFAULT: [Strategy; 4] =
        [Strategy::Optimal, Strategy::Fess, Strategy::Backward(1), Strategy::Backward(0)];

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "optimal" => Strategy::Optimal,
            "backward" => Strategy::Backward(1),
            _ if name.starts_with("backward:") => Strategy::Backward(name["backward:".len()..].parse().ok()?),
            "fess" => Strategy::Fess,
            "backward-greedy" => Strategy::Backward(0),
            _ => return None,
        })
    }

    fn name(&self) -> &'static str {
        match self {
            Strategy::Optimal => "fwd-optimal",
            Strategy::Backward(0) => "backward-greedy",
            Strategy::Backward(1) => "backward",
            Strategy::Backward(_) => "backward-weighted",
            Strategy::Fess => "fess",
        }
    }

    fn run(&self, board: &Board, opts: &Options) -> Outcome {
        match self {
            Strategy::Optimal => solver::solve(board, &Options { mode: Mode::OptimalPushes, ..opts.clone() }),
            Strategy::Backward(w) => {
                let mode = match w { 0 => Mode::Greedy, 1 => Mode::OptimalPushes, _ => Mode::Weighted(*w) };
                solver::solve_backward(board, &Options { mode, ..opts.clone() })
            }
            Strategy::Fess => fess::solve(board, opts),
        }
    }
}

/// Racing portfolio (Sokolution-style): every strategy runs in its own
/// thread with the FULL time budget; the first SOLUTION stops the others.
/// Returns the outcome and which strategy produced it; earlier strategies
/// win ties, so with forward-optimal first, reported solutions are
/// push-optimal whenever it finished in time.
///
/// An "unsolvable" verdict (a strategy exhausted its search space) only ends
/// that strategy's thread: solutions are verified by replay, verdicts are
/// not, so one unsound strategy must not be able to stop the rest (a buggy
/// deadlock table once made every level of a benchmark "unsolvable" this
/// way). A solution found after another thread claimed "unsolvable" proves
/// a soundness bug and is reported loudly.
fn solve_auto(board: &Board, base: &Options, strategies: &[Strategy]) -> (Outcome, &'static str) {
    use std::sync::atomic::Ordering;
    let outcomes: Vec<Outcome> = std::thread::scope(|s| {
        let threads: Vec<_> = strategies
            .iter()
            .map(|strategy| {
                let opts = base.clone();
                s.spawn(move || {
                    let outcome = strategy.run(board, &opts);
                    if matches!(outcome, Outcome::Solved { .. }) {
                        opts.stop.store(true, Ordering::Relaxed);
                    }
                    outcome
                })
            })
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).collect()
    });

    if outcomes.iter().any(|o| matches!(o, Outcome::Solved { .. })) {
        for (o, strategy) in outcomes.iter().zip(strategies) {
            if matches!(o, Outcome::Unsolvable { .. }) {
                eprintln!(
                    "WARNING: {} claimed {} unsolvable but another strategy solved it: soundness bug",
                    strategy.name(),
                    board.name
                );
            }
        }
    }
    let rank = |o: &Outcome| match o {
        Outcome::Solved { .. } => 0,
        Outcome::Unsolvable { .. } => 1,
        Outcome::Exhausted { .. } => 2,
    };
    outcomes
        .into_iter()
        .zip(strategies.iter().map(Strategy::name))
        .min_by_key(|(o, _)| rank(o))
        .expect("at least one strategy")
}

/// Memory watchdog: runs `solve` while a background thread polls the
/// process's resident memory; past `limit_bytes` it raises `stop`, which
/// every search checks periodically, so an over-budget search ends as
/// "exhausted" instead of exhausting the machine.
fn with_memory_limit<T: Send>(
    limit_bytes: u64,
    stop: &std::sync::atomic::AtomicBool,
    solve: impl FnOnce() -> T + Send,
) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if resident_bytes().is_some_and(|rss| rss > limit_bytes) {
                    stop.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let result = solve();
        done.store(true, Ordering::Relaxed);
        result
    })
}

/// Resident memory of this process (Linux /proc; None elsewhere).
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    kb_field(&status, "VmRSS:").map(|kb| kb * 1024)
}

/// Default memory limit: half of physical RAM (unlimited if unknown).
fn default_memory_limit() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| kb_field(&m, "MemTotal:"))
        .map_or(u64::MAX, |kb| kb * 1024 / 2)
}

fn kb_field(text: &str, name: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with(name))?;
    line[name.len()..].trim().trim_end_matches("kB").trim().parse().ok()
}

/// Diagnostic: the level map with each goal replaced by its packing layer
/// (0 = fill first; letters after 9).
fn print_plan(board: &Board) {
    let t = std::time::Instant::now();
    let plan = packing::PackingPlan::compute(board);
    let elapsed = t.elapsed();
    let t = std::time::Instant::now();
    let _ = deadsets::DeadSetTables::new(board, deadsets::Direction::Forward);
    println!("dead-set tables: {:.1?}", t.elapsed());
    let t = std::time::Instant::now();
    let relaxed = retro::RelaxedPlan::compute(board, 20_000);
    match &relaxed {
        Some(r) => {
            let parks = r.steps.iter().filter(|s| s.from.is_some()).count();
            println!("relaxed plan: {} steps ({} moves of placed boxes) in {:.1?}", r.steps.len(), parks, t.elapsed());
        }
        None => println!("relaxed plan: none within budget ({:.1?})", t.elapsed()),
    }
    println!(
        "{}: {} layers {:?}, exact tables {:?} ({elapsed:.1?})",
        board.name,
        plan.num_layers(),
        plan.sizes,
        plan.table_sizes()
    );
    for y in 0..board.height {
        let row: String = (0..board.width)
            .map(|x| {
                let sq = board.sq_index[y * board.width + x];
                if sq == level::NONE {
                    return '#';
                }
                let s = sq as usize;
                if board.is_goal[s] {
                    let l = plan.layer[s] as u32;
                    return std::char::from_digit(l.min(35), 36).unwrap();
                }
                if board.start_boxes.contains(&sq) {
                    '$'
                } else if sq == board.start_player {
                    '@'
                } else {
                    ' '
                }
            })
            .collect();
        println!("{row}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: sokoban-solver <levels.txt> [--level N] [--time-limit SECS]\n\
             \x20 [--portfolio optimal,backward,backward:W,backward-greedy,fess]   (default: optimal,fess,backward,backward-greedy)\n\
             \x20 [--mode auto|optimal|greedy|weighted:W|backward|fess]\n\
             \x20 [--memory-limit GB]   (default: half of RAM)  [--max-nodes N] [--no-corral]\n\
             \x20 [--solutions FILE] [--quiet] [--show-plan]"
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
    let mut show_plan = false;
    let mut portfolio: Vec<Strategy> = Strategy::DEFAULT.to_vec();
    let mut memory_limit = default_memory_limit();

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
            "--memory-limit" => {
                let gb: f64 = it.next().expect("--memory-limit GB").parse().expect("gigabytes");
                memory_limit = (gb * 1e9) as u64;
            }
            "--show-plan" => show_plan = true,
            "--portfolio" => {
                let list = it.next().expect("--portfolio a,b,...");
                portfolio = list
                    .split(',')
                    .map(|n| Strategy::parse(n).unwrap_or_else(|| panic!("unknown strategy {n}")))
                    .collect();
            }
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
        if show_plan {
            print_plan(&board);
            continue;
        }
        opts.stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (outcome, tag) = with_memory_limit(memory_limit, &opts.stop, || {
            if auto {
                solve_auto(&board, &opts, &portfolio)
            } else if backward {
                (solver::solve_backward(&board, &opts), "backward")
            } else if fess_mode {
                (fess::solve(&board, &opts), "fess")
            } else {
                (solver::solve(&board, &opts), "single")
            }
        });
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
