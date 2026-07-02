# sokoban-solver

A high-performance Sokoban solver in Rust, built on the techniques used by the
strongest known solvers (Festival/FESS, Sokolution, YASS, Rolling Stone, JSoko).

## Usage

```
cargo build --release
./target/release/sokoban-solver levels/microban1.txt                  # solve all levels (auto portfolio)
./target/release/sokoban-solver levels/microban1.txt --level 42      # one level
./target/release/sokoban-solver levels/xsokoban.txt --time-limit 60  # per-level budget (seconds)
./target/release/sokoban-solver f.txt --mode optimal                 # push-optimal A* only
./target/release/sokoban-solver f.txt --mode backward                # pull search from the goal state
./target/release/sokoban-solver f.txt --mode fess                    # feature-space search
./target/release/sokoban-solver f.txt --mode weighted:3              # weighted A*
./target/release/sokoban-solver f.txt --mode greedy                  # greedy best-first
./target/release/sokoban-solver f.txt --solutions out.sok --quiet    # write LURD solutions
./bench.sh auto 10                                                    # benchmark all Microban sets
```

Default mode is `auto`: a racing portfolio (Sokolution-style) — forward
push-optimal A*, bidirectional meet-in-the-middle (g+h for half the budget,
then g+2h), weighted A* (w=3 then w=5), and FESS each run in their own
thread with the full time budget; the first definite answer stops the rest.
The tag in the output shows which strategy won. Forward-optimal wins ties,
so reported solutions are push-optimal whenever that search finished in
time; a strategy that exhausts its complete search space proves the level
unsolvable.

Every solution is verified twice before being reported: reconstructed into a
full LURD move string (player walking recovered by BFS), then replayed against
the original unmodified level text.

## Architecture

- **State** = (sorted box squares, canonical player square), where the player
  square is normalized to the minimum-index square of its reachable region.
  Push-based search: the player's walking between pushes is irrelevant to
  state identity and is reconstructed afterwards.
- **Search** (`solver.rs`): best-first over pushes with lazy child
  materialization — heap entries store (parent, push); a child is only
  materialized (reachability BFS + transposition check + arena node) when
  popped. With the consistent matching heuristic, the first pop of a state has
  minimal g, so A* optimality is preserved with pop-time dedup.
- **Heuristic** (`matching.rs`): minimum-cost perfect matching of boxes to
  goals over relaxed push distances (per-goal backward pull BFS). Admissible
  and consistent (1-Lipschitz per push). Computed once per expanded node
  (Jonker-Volgenant with potentials), then each child's value by a single-row
  re-augmentation from a snapshot (dynamic Hungarian, Mills-Tettey & Stentz)
  — O(n·m) per child instead of O(n³). Rectangular instances are padded with
  zero-cost dummy rows so the matching stays square and perfect (this is what
  makes single-row re-augmentation exact; see the property tests).
  Infeasible matching = a box can no longer reach any free goal = deadlock.
- **Deadlock detection**:
  - *Dead squares* (`level.rs`): per-goal pull BFS (a pull needs the target
    square AND the player square beyond it free); squares from which no goal
    is reachable are dead — no push onto them is ever generated.
  - *Freeze deadlocks* (`deadlock.rs`): recursive two-axis blocked test with
    the on-path boxes treated as walls (sound because pushes are sequential:
    a cycle of mutual blocking means no box can move first). A frozen cluster
    containing an off-goal box kills the position; a box frozen on a goal
    triggers a cluster re-check.
  - *PI-corral pruning* (`corral.rs`): when a player-unreachable region is
    fenced so that — even with all outside boxes removed — every possible
    first push of a fence box goes inward and is executable right now, only
    those pushes are generated at this node ("the alpha-beta of Sokoban",
    Damgaard/Meger). Verified optimality-preserving by A/B testing push
    counts across the Microban sets. An unfinished corral with *no* possible
    push is a proven deadlock.
  - *Corral deadlock mini-search* (`corral.rs`): corrals that fail the PI
    conditions get a small capped sub-search on the reduced board; if the
    corral's boxes provably can never resolve it — with conservative bails
    for a box exiting the closure, the player breaking in, or an outside box
    becoming insertable through an opened fence — the node is a proven
    deadlock. Verdicts are cached per corral pattern.
  - *Frozen-as-walls matching* (`deadlock.rs::scan_frozen`): boxes frozen on
    goals have their matching row locked to that goal (they can never move
    again — exact, not just admissible), and a frozen box off a goal kills
    the node. Catches "a frozen box occupies the only goal another box can
    reach" deadlocks. Consistency is preserved (freezing is permanent), so
    A* optimality is unaffected — verified by push-count A/B.
- **Backward search** (`solver.rs::solve_backward`): pulls boxes from the
  goal-filled board back to the start squares, one root per player region of
  the goal state; mirrors the forward machinery (start-square distance
  tables, backward dead squares, matching bound). Cramped-goal levels that
  are hopeless forward often fall instantly backward. Exhausting the pull
  space proves unsolvability.
- **Bidirectional meet-in-the-middle** (`bidir.rs`): a forward push search
  and a backward pull search interleaved in one thread, cross-probing each
  other's transposition tables. Keys are identical on both sides — equal
  boxes and equal player region — so a hit splices the forward path with the
  reversed pull suffix into a full (non-optimal) solution at roughly half
  the search depth per side. Probe-only: frontiers are never merged. Solved
  mb2 #126, which resisted every unidirectional strategy at 60 s and YASS.
- **FESS-lite** (`fess.rs`): feature-space search after Shoham & Schaeffer's
  FESS (Festival). Nodes project onto cells of a 2-D feature space (boxes on
  goals, player-region connectivity); the search cycles over active cells,
  expanding one lowest-weight pending move per cell per round; advisor moves
  (pack a box / improve connectivity) get weight 0, others weight 1. No
  admissible heuristic needed; solves dense rearrangement levels that defeat
  A* orderings.

## Benchmarks (Microban I–IV, 493 levels, 4-core box)

| Set | Levels | This solver @10s | This solver @60s | YASS 2.153 @10s |
|-----|--------|------------------|------------------|-----------------|
| Microban I | 155 | 154 | 154 | 155 |
| Microban II | 135 | 132 | 133 | 131 |
| Microban III | 101 | 99 | 100 | 99 |
| Microban IV | 102 | 96 | 98 | 92 |
| **Total** | **493** | **481** | **485** | **477** |

Three of the eight levels still failing at 60 s (mb1 #153, mb2 #130/#131)
fall between 1 and 3 minutes; the stubborn residue is mb3 #58 and
mb4 #57/#59/#75/#85. XSokoban 90 at 10 s/level: 16/90 (doubled by the
bidirectional search and corral mini-search) — cracking that suite needs the
stage-3 packing-plan machinery (Rolling Stone reached 59/90; Festival
90/90).

YASS 2.153 (open source, by the co-inventor of PI-corral pruning) was built
from source with FreePascal and run on the same machine as the reference
baseline. The two solvers fail on *different* levels: together they solve
487/493. Only six levels resist both (mb2 126/131, mb3 58, mb4 59/75/85) —
dense multi-box rearrangement puzzles that are genuinely hard for classical
search at these budgets; Festival-class packing plans are the known remedy.

## Lessons encoded in the code

- The freeze checker's treat-as-wall marks must be scoped to the recursion
  path; stale marks cause false deadlocks.
- Single-row re-augmentation of a rectangular Hungarian problem is *not*
  sound without square padding (complementary slackness requires free
  columns at potential 0); a property test pins this.
- Naive tunnel macros (force a box through wall-flanked goal-free runs) are
  **unsound**: a box may need to park inside a tunnel purely to vacate its
  previous square for player passage (microban1 #10; regression test).
  Reverted; revisit only with Rolling Stone's exact formulation.
- Unreachable `*` squares are decorative (box parked on goal forever) and
  must be dropped from the puzzle, not rejected (microban1 #155).
- The independent LURD verifier must pad ragged rows with wall exactly like
  the board builder, or void cells replay as floor (found by adversarial
  review; regression test).

## Ideas beyond the literature (implemented and planned)

Already in: a 4-way racing portfolio (known solvers race at most 2
strategies), a two-stage weighted slot (different weights crack different
levels — the search is chaotically sensitive to tie-breaking), and
symmetry-reduced transposition keys over board automorphisms (sound,
optimality-preserving, and apparently absent from the classical solvers).

Planned, in rough order of expected value:

1. ~~Bidirectional meet-in-the-middle~~ — implemented (`bidir.rs`), in the
   default portfolio; worth +3–4 levels on Microban.
2. **Macro-FESS + packing plans** (Festival stage 3): per-box push-path macro
   moves, sink-room packing order via backward pull-FESS, goal-room packing
   tablebases (exact retrograde enumeration of small goal rooms). The known
   remedy for XSokoban-class levels.
3. **Parallel FESS**: the cyclic cell scan parallelizes naturally across
   cells (Festival is single-threaded — a 32-core FESS is unexplored
   territory).
4. **Persistent cross-level deadlock pattern store**: minimized corral
   deadlock patterns, translation-invariant, learned once and reused across
   a whole level set.
5. **Restart-diverse portfolios**: many short randomized searches (perturbed
   tie-breaks) instead of one long one, exploiting the huge run-to-run
   variance observed on hard levels.
6. **RL, final stage** (per project direction, after classical techniques
   are exhausted): a learned move-ordering/advisor policy (small CNN) plugged
   into FESS as an additional advisor — strictly heuristic guidance, never a
   pruner, so soundness and completeness are unaffected. Training data:
   solutions this solver already produces; benchmark: boxoban-hard.
