# sokoban-solver

A Sokoban solver in Rust. It races several complementary searches, the
strongest of which is a feature-space search (after Festival's FESS) over
macro moves, guided by packing plans computed backward from the goal, with
a stack of provably sound deadlock detectors. Every technique here was
re-derived from first principles, measured on real levels, and kept only if
it helped; several that did not are listed at the end.

## Usage

```
cargo build --release
./target/release/sokoban-solver levels/microban1.txt                  # solve all levels (portfolio)
./target/release/sokoban-solver levels/microban1.txt --level 42      # one level
./target/release/sokoban-solver levels/xsokoban.txt --time-limit 60  # per-level budget (seconds)
./target/release/sokoban-solver f.txt --portfolio optimal,bidir,fess      # choose the racing strategies
./target/release/sokoban-solver f.txt --mode optimal                 # push-optimal A* only
./target/release/sokoban-solver f.txt --mode fess                    # feature-space search only
./target/release/sokoban-solver f.txt --mode backward                # pull search from the goal
./target/release/sokoban-solver f.txt --memory-limit 2               # GB (default: half of RAM)
./target/release/sokoban-solver f.txt --solutions out.sok --quiet    # write LURD solutions
./target/release/sokoban-solver f.txt --show-plan                    # print packing plans (diagnostic)
```

Every solution is reconstructed into a LURD move string and replayed
against the original level text before it is reported.

## How it works

### Portfolio (`main.rs`)

Four threads race with the full time budget each; the first *solution*
stops the others. Default: push-optimal A\* (reported solutions are optimal
whenever it finishes first), bidirectional search, FESS, and backward (pull)
search. The portfolio was chosen by measuring each strategy's *unique*
solves (weighted A\* is available with `--portfolio` but added none).

An "unsolvable" verdict only ends the thread that reached it: solutions are
verified by replay, verdicts are not, so one unsound strategy cannot stop
the rest. A solution found after another thread claimed "unsolvable" is
reported as a soundness bug. A memory watchdog polls the process's resident
memory and stops the searches (reported as a timeout) at the limit.

### Feature-space search over macro moves (`fess.rs`, `macros.rs`)

Best-first search orders positions by an estimate of remaining work, which
is uninformative for rearrangement puzzles. FESS instead projects positions
onto a small feature space — (boxes packed per the plan, free-space
regions) — and cycles over occupied cells, expanding one move per cell per
visit, so progress in any feature earns its own share of effort. Within a
cell, moves are taken by accumulated weight: advisor moves (the best move
that packs a box, merges free regions, or reduces the number of boxes
standing in other boxes' way) cost 0, others 1.

Moves are **macro moves**: one box pushed any number of times while the
player walks freely. Per box, a breadth-first search over (box square,
player side); which sides the player can walk between while the box sits on
a square comes from one depth-first search (articulation points) per box,
and so does each move's free-region count — O(squares) per box instead of a
flood fill per reached square. A brute-force property test pins both.

### Packing plans (`packing.rs`, `retro.rs`)

*In which order must the goals be filled?* Backward from the solved
position, the goal filled last must be one whose box can still be pulled
out to where some box starts, with every other goal full. Removal is
monotone, so peeling off every removable box in rounds gives a layering
without search; it is computed per cluster of adjacent goals. For small
clusters the plan keeps the exact set of *consistent* fillings (those from
which the cluster can still be completed), found by the same retrograde
removal.

Orders cannot express **parking** — a box pushed out of a goal area into a
niche and brought back later (XSokoban #44). A relaxed backward solve
(pull boxes from the goals with macro pulls; a box that reaches a start
square disappears; other boxes are ignored) yields a step-by-step plan in
milliseconds; on levels whose plan parks boxes, FESS measures progress along
that plan instead.

### Deadlock detection (all sound)

- *Dead squares* (`level.rs`): squares from which no goal is reachable.
- *Freeze deadlocks* (`deadlock.rs`): boxes that can never move again.
- *Frozen boxes as walls* (`deadlock.rs`): a box frozen on a goal is a wall
  for every other box; distances recomputed with those walls prove "all but
  one packed, the last goal sealed off" positions dead and strengthen the
  A\* lower bound. Tables are cached per frozen set.
- *Small box-set tables* (`deadsets.rs`): for every pair (and, on small
  levels, triple) of box squares and player region, whether those boxes
  alone can still be finished — exact, by retrograde search from the goals.
  Removing boxes only makes a position easier, so a dead set is dead in any
  position. The mirror image (seeded from start squares, expanded by pushes)
  prunes the backward searches. Checked against exhaustive search and
  against real solutions in both directions.
- *PI-corrals and corral mini-search* (`corral.rs`).
- *Gate pushes* (`level.rs`): after a box is pushed onto a non-goal square
  that (with walls alone) cuts the pushing side off from the box's other
  sides, only the forward push of that box is generated.

### Other searches

`solver.rs`: A\* over pushes with the min-cost matching lower bound
(incremental Hungarian, `matching.rs`), and the backward (pull) search.
`bidir.rs`: forward and backward searches probing each other's
transposition tables; it expands whichever side has the smaller frontier.

## Benchmarks

Per-level time limit 10 s, 4-core machine, full default portfolio.

| Set | Levels | This solver | Same machine: YASS 2.153 | Published @10 s: Festival / Sokolution* |
|-----|--------|-------------|--------------------------|------------------------------------------|
| Microban I–IV | 493 | **486** | 477 | — |
| XSokoban | 90 | **67** | — | 88 / 85 |
| SokEvo | 107 | **107** | — | 107 / 107 |
| Grigr2001 | 100 | **93** | — | 96 / 96 |
| Holland | 81 | **59** | — | 65 / 68 |
| Sasquatch | 50 | **29** | — | 41 / 43 |
| SokHard | 163 | **98** | 117 | 134 / 163 |

\* From the sokobano.de solver statistics (Large Test Suite), measured on a
Ryzen 9 7900X with 8+ threads — not directly comparable. YASS was built from
source and run single-threaded on the same 4-core machine as this solver.

Progress in the September 2026 session (same machine, 10 s): XSokoban
16 → 67, Microban 480 → 486, SokHard 61 → 98. The remaining gap is on small,
dense levels (SokHard, Sasquatch), where YASS reaches solutions with far
fewer search nodes; larger deadlock sets and stronger lower bounds are the
next things to try there.

## Open problems and next steps

- **Small, dense levels** (SokHard, Sasquatch) are the gap: YASS reaches
  solutions with 10-100x fewer search positions. A profile of A\* on failing
  SokHard levels shows no dominant cost (children 35-38%, frozen scan +
  matching ~22%, materialization ~15%, heap ~15%, corral ~9% at ~40k
  expansions/s), so engine speed-ups (compact states, incremental hashing)
  would give ~2x at most — yet a 60 s run solves 66 of the 74 SokHard levels
  failed at 10 s (median 24.5 s), so a 2-3x faster engine would close much
  of the gap. Also: deadlock patterns learned from exhausted subtrees,
  YASS-style perimeter search.
- **Large levels** (XSokoban at 67/90): FESS features beyond packing,
  regions and hotspots (room connectivity, out-of-plan boxes), and sharing a
  forward/backward meet table across threads.
- **Reinforcement learning** only after the classical techniques are
  exhausted, and only as an advisor inside FESS (never a pruner).

## Experiments

Every technique tried — kept or not — with its reasoning and measurement is
in [EXPERIMENTS.md](EXPERIMENTS.md).

## Lessons encoded in the code

- The freeze checker's treat-as-wall marks must be scoped to the recursion
  path; stale marks cause false deadlocks.
- Single-row re-augmentation of a rectangular Hungarian problem is *not*
  sound without square padding; a property test pins this.
- Naive tunnel macros are **unsound**: a box may need to park inside a
  tunnel to vacate a square for the player (microban1 #10; regression test).
  The gate-push rule is sound because it depends on the position alone.
- Board symmetries preserve walls and goals but not start squares: the
  backward search must not merge mirror-image states.
- A corral verdict must be cached per corral *region*: one fence can enclose
  several regions with different verdicts.
- A packed-box feature must count a goal only once the goals behind it are
  full, per goal cluster; and "a box can leave the goal area" must mean it
  can reach a start square, not merely a non-goal square (XSokoban #44).
- Unreachable `*` squares are decorative and must be dropped, not rejected
  (microban1 #155). The verifier must pad ragged rows like the board builder.
