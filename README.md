# sokoban-solver

A Sokoban solver in Rust. It races several complementary searches, the
strongest of which is a feature-space search (after Festival's FESS) over
macro moves, guided by packing plans computed backward from the goal, with
a stack of provably sound deadlock detectors. Every technique here was
re-derived from first principles, measured on real levels, and kept only if
it helped; several that did not are listed at the end.

An illustrated guide to how the solver works, with diagrams and links into
the code: [Sokoban Solver Field Notes](https://rkirov.github.io/sokoban-ai/)
(`docs/index.html`).

## Usage

```
cargo build --release
./target/release/sokoban-solver levels/microban1.txt                  # solve all levels (portfolio)
./target/release/sokoban-solver levels/microban1.txt --level 42      # one level
./target/release/sokoban-solver levels/xsokoban.txt --time-limit 60  # per-level budget (seconds)
./target/release/sokoban-solver f.txt --portfolio optimal,fess,backward:2 # choose the racing strategies
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
whenever it finishes first), FESS, and backward (pull) search twice —
optimal and greedy (f = h) — because the two orderings solve different
cramped levels. The portfolio was chosen by measuring each strategy's
*unique* solves and then head-to-head on every pack (bidirectional search
and staged weighted forward A\* were removed: they added no solves).

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
(incremental Hungarian, `matching.rs`), and the backward (pull) search,
optimal, weighted or greedy.

## Benchmarks

Per-level time limit 10 s, 4-core machine, full default portfolio.

| Set | Levels | This solver | Same machine: YASS 2.153 | Published @10 s: Festival / Sokolution* |
|-----|--------|-------------|--------------------------|------------------------------------------|
| Microban I–IV | 493 | **488** | 477 | — |
| XSokoban | 90 | **67** | — | 88 / 85 |
| SokEvo | 107 | **107** | — | 107 / 107 |
| Grigr2001 | 100 | **93** | — | 96 / 96 |
| Holland | 81 | **61** | — | 65 / 68 |
| Sasquatch | 50 | **32** | — | 41 / 43 |
| SokHard | 163 | **146–147** | 117 | 134 / 163 |

\* From the sokobano.de solver statistics (Large Test Suite), measured on a
Ryzen 9 7900X with 8+ threads — not directly comparable. YASS was built from
source and run single-threaded on the same 4-core machine as this solver.

Progress since September 2026 (same machine, 10 s): XSokoban 16 → 67,
Microban 480 → 488, SokHard 61 → 146–147 (runs vary by a few levels on
this shared machine), Sasquatch 26 → 32. On SokHard this is ahead of YASS
(117 on the same machine), Festival (134) and Takaken (139) as published
on a faster machine; only Sokolution (163) solves more.

## Open problems and next steps

- **Large levels** (XSokoban 67/90, Holland 61/81): not time-bound — a 60 s
  run solves only 4 of XSokoban's 23 misses. FESS's best positions on the
  misses are provably dead (all but 2–4 boxes packed in a way that cannot
  be completed), so the lever is recognising those dead ends early:
  deadlock patterns learned from stalled positions (proofs currently cost
  2–30 s, too slow at 10 s), and FESS features beyond packing, regions and
  hotspots.
- **Dense levels** (Sasquatch 29/50, SokHard misses): mostly time-bound —
  most SokHard misses solve within 60 s, largely by the backward threads —
  so backward-search speed keeps paying off.
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
