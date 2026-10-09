# Experiments log

Every technique tried on this solver, including the ones that are not in
it, with the reasoning and the measurement that decided it.

**Method.** Unless stated otherwise: 10 s per level, 4-core machine, the
full default portfolio, counted as levels solved. Every solution is
replayed against the level text. A technique is kept only if it adds
solves on the real problem packs (Microban I–IV, XSokoban, Large Test Suite
sets) beyond run-to-run noise; when results are equal, the simpler code
wins.

**Noise.** The machine is shared (other agent sessions, memory pressure),
so wall-clock portfolio runs on SokHard vary by about ±5–6 levels between
identical builds (the same build measured 94, 97 and 98). Changes that
alter the search are therefore also compared with deterministic
node-limited runs of the affected thread; pure speed-ups must first
reproduce identical node counts, then are judged by total time. Pruning rules must also leave optimal push
counts unchanged level by level (A\*, Microban I–IV).

Packs: Microban I–IV (493), XSokoban (90), and from the Large Test Suite
SokHard (163), Holland (81), Sasquatch (50), Grigr2001 (100), SokEvo (107).
SokHard and the other LTS sets were never used for tuning until measured.

## Summary of the September 2026 session

| Set | Start | End |
|-----|-------|-----|
| XSokoban | 16 | 67 |
| Microban I–IV | 480 | 488 |
| SokHard | 61 (first measured) | 115–116 |
| Holland / Sasquatch / Grigr2001 / SokEvo | 57 / 26 / 90 / 107 | 61 / 29 / 93 / 107 |

Same-machine reference: YASS 2.153 solves 117 SokHard levels at 10 s
(single-threaded), and 477 Microban levels.

## Bugs found

| Bug | How found | Effect |
|-----|-----------|--------|
| Backward search merged mirror-image states (symmetries don't preserve start squares) | reproduced: a one-push symmetric level reported UNSOLVABLE | fixed, regression test; portfolio unaffected (it didn't run that search) |
| Corral verdict cache keyed without the corral region | instrumentation: ~1.5k cache hits/Microban II with a verdict from another region | fixed; results unchanged (no wrong answer found) |
| Dead-set refactor rejected every backward push transition | the whole portfolio collapsed (SokHard 1/163) via one false "unsolvable" | fixed; solution-replay test and the only-solutions-stop policy added |

## Feature-space search (FESS) and plans (XSokoban, FESS alone)

| Technique | Why | Result | Status |
|-----------|-----|--------|--------|
| FESS rewritten over macro moves (one box, any pushes) with cells (packed, free regions), packing + region-merge advisors | depth = box moves, not pushes; features give every kind of progress its own effort | 16 → 47 (old FESS was unit-push) | kept |
| Goal layers per goal *cluster* (not global) | an independent goal pocket landed in the last layer and never counted (XSokoban #10) | #10 solved in 0.18 s | kept |
| "Can leave" = pulled back to a *start square* (not any non-goal square) | a niche reachable only from the goal counted as leaving (XSokoban #44) | 47 → 51 | kept |
| Exact consistent-filling tables per cluster (with per-box credit for the never-removable core) | layers lose order information within a layer | 51 → 52, 30% faster | kept (cap 4k states, 9% faster than 20k) |
| Relaxed backward plan with parking, used as progress when the plan parks boxes | orders cannot express parking (#44 fills a 3×3 block through niches) | 52 → 59 | kept |
| Relaxed-plan progress on every level | — | 58 (−1) | rejected: a fixed sequence is more restrictive than the partial order |
| Plan removals only at start squares ≥ 3 / ≥ 6 steps from goals | fetch boxes from where they actually are | 59 / 60 (noise) | rejected; as a second FESS thread +2 XSokoban once, later not in the default and removed |
| Hotspots: boxes that reduce other boxes' reachable goals (advisor + tie-break) | preparation moves get no packing credit | 60 → 62 → 63 (with bitset counting) | kept |
| O(1) per-move region counts from articulation data | region count per child was the top cost | 2× faster, +1 | kept |
| Boxes frozen on goals as walls (distances, matching) | "all but one packed, last goal sealed" was undetected | +1, 40% faster | kept (FESS and A\*) |
| Alternate cyclic cell visits with best-cell visits | concentrate effort | 47 = 47 | rejected (neutral) |
| Free moves at nodes with ≤ 2 moves (Festival) | near-forced choice | −2 | rejected |
| Prefer boxes farther from goals as tie-break (Festival) | avoid crowding | −2 | rejected |
| FIFO vs LIFO in-cell tie-break | — | 59 = 59 | rejected (neutral) |
| Greedy "move boxes closer to goals" advisor | avoid depth = breadth-first when no advisor fires | −6 | rejected |
| Hand near-complete FESS positions to budgeted weighted A\* | A\* is strong at tight endings | −6 (20k nodes), 19/90 (100k) | rejected |
| Backward breadth-first endgame table probed by FESS | perimeter search | −1, 2× slower: stalled positions were dead, not short | rejected |

## Portfolio

| Change | Result | Status |
|--------|--------|--------|
| Weighted A\* thread (w = 3 then 5) | no unique Microban solves | opt-in, later removed |
| Backward (pull) A\* replacing the second FESS thread | SokHard +3, Microban +1 | kept |
| Bidir expands the smaller frontier (Pohl) instead of alternating | SokHard +6 | kept, then bidir itself removed (below) |
| Weighted backward thread (w = 3) *replacing* optimal backward | alone 70 vs 61, portfolio 97 → 87 | rejected: solo strength ≠ portfolio value |
| Weighted backward (w = 3) *alongside* optimal backward, replacing bidir (chosen from unions of solo runs: 100 vs 95 on SokHard) | head-to-head on every pack: Microban 486 → 488, Holland 59 → 61, SokHard 99/100 → 103/98 (noise), others equal | kept as default |
| Bidir forward side with A\*'s gate and frozen-wall rules | bidir alone 77 → 84, portfolio neutral | reverted |
| Size-adaptive portfolio: a second, differently planned FESS on large levels (FESS wins 63/67 XSokoban solves) | upper bound first: FESS variants alone on the 23 XSokoban misses solve 1 (far-removal plans) / 0 (plan progress everywhere) / 0 (control) | not built |
| Bidirectional search (the earlier session's meet-in-the-middle, 500 lines) | out of the default once weighted backward replaced it with no loss on any pack | removed (simplicity) |
| Only a verified solution stops the portfolio; contradicting "unsolvable" claims reported | safety | kept |
| Memory watchdog (process RSS, default half of RAM) | no machine OOM at long limits; same results | kept |

## Deadlock detection and bounds (SokHard unless stated)

| Technique | Why | Result | Status |
|-----------|-----|--------|--------|
| Exact pair deadlock table (retrograde BFS over pair + player region) | small dense levels full of 2-box deadlocks | A\* nodes −16–20% on Microban, identical optimal pushes; SokHard 61 → 71 with the frontier rule | kept |
| Backward (mirror-image) pair table for pull searches | backward search had almost no deadlock detection | 71 → 77 | kept |
| Triple tables + gate forced pushes (A\* only) | 3-box jams; gate pushes are position-only, so TT-safe | 77 → 86 | kept |
| Quadruple tables (≤ 50 squares) | more deadlocks | 36% fewer nodes, 84 vs 85 | rejected |
| Partner-square filtering of dead-set checks | lookups were the largest per-child cost | 89 → 98 | kept |
| Pair lower bound: sum of exact two-box distances over disjoint pairs (admissible: every push moves one box) | stronger bound on dense levels | 14–28% fewer nodes, slower; A\*-only 62 vs 69 | rejected |
| Naive tunnel macros (earlier session) | fewer nodes | unsound (Microban I #10 needs parking in a tunnel) | rejected; gate rule used instead |

## Engine speed

| Change | Result | Status |
|--------|--------|--------|
| Bucket open list (identical order) | identical node counts; backward 10–15% faster, 4× smaller queue entries; SokHard portfolio 94 vs 83 without it (noisy) | kept |
| Transposed distance tables | identical nodes, slightly slower | rejected |
| Warm-start matching from the parent's saved state (backward) | 17–25% faster; with one backward thread the portfolio showed 94 vs 97 (noise) | dropped at first |
| Same warm start, re-tested once the default ran two backward threads | SokHard 101/103 → 116/115 (two interleaved runs each), Holland 60 → 61, Microban IV equal and faster (median ×0.72) | kept |
| Warm-start matching (forward A\*), re-tested | identical optimal pushes, optimal A\* 5–20% faster; portfolio SokHard 118/117 vs 117/116, Holland and Microban equal | rejected: within noise, ~40 lines |

## Ideas from Codex (asked for novel first-principles ideas)

| Idea | Assessment | Status |
|------|------------|--------|
| Seed backward dead-set tables only from the actual start-player region | sound (projection of a real solution) and free | backward A\* with a fixed 400k-node budget on SokHard: 47 → 48, identical optimal pushes; wall-clock portfolio too noisy to resolve | kept (7 lines) |
| Dual-based O(m) child lower bounds, exact matching only when popped (backward search) | sound (weak duality); identical optimal pushes | backward alone +3 Microban levels, faster on 3 of 4 sets; portfolio SokHard 117/116 vs 114/116, Holland equal and 8% faster | kept |
| Backward "freeze": erase pullable boxes to a fixed point; leftovers never move | sound (erasure frees more than any placement); a solution-replay test confirmed no false positives | fixed 400k-node budget on SokHard 48 → 49, but backward 27% slower per node and no Microban gains | rejected |
| Generation-time duplicate rejection in backward search (TT keyed by box set; flood the child's region only when its box set was seen) | sound; 60–70% of backward pops were duplicates and ~80% of those are catchable at generation | identical optimal pushes; identical time at a fixed 400k-node budget (the child floods cost what the saved pops did); backward alone on Microban −2 levels; portfolio SokHard 117/117 vs 116/116, Holland 61 = 61 | rejected: within noise, ~40 lines |
| Player-side-aware single-box distances (BFS over box square × player-side component; min over sides, so a drop-in table), also for the frozen-wall tables | sound; tighter on 70/163 SokHard and 50/90 XSokoban levels; on dumped dead FESS positions the frozen-wall version proves 66/227 dead at 0 nodes (11 before) | optimal A\* pushes identical on all Microban; with the greedy-backward portfolio: SokHard 126/124 vs 125/124, Sasquatch 31 vs 32, Holland, XSokoban, Microban IV equal | rejected: within noise, ~150 lines (branch `player-aware`) |
| FESS advisors for reopening goals / two-move preparation witnesses | speculative | not tried |
| Learned deadlock patterns (Festival "stuck" patterns): shrink free boxes, ≤500-state macro proof search, greedy minimisation, store (boxes, player region), match on every pop and child | sound (box removal only relaxes; replay test: never learns from solution positions) | FESS alone XSokoban: subtree-64 trigger 67 = 67 (learning 70% of time at first); top-layer trigger + work cap 67 = 67 (+#11, −#44); both triggers 65. Root proofs on top-layer positions: with free-box shrinking 75–95% come out *alive* (shrinking deletes the boxes that cause the deadlock, ending with an empty, "solved" board); without shrinking 95% exhaust the budget even with frozen walls, player-aware distances and corral pruning in the prover. | rejected for now (branch `learned-patterns-wip`) |
| Greedy backward thread (f = h, pulls) replacing weighted backward (w = 3) | Sokolution runs greedy searches by default; SokHard is time-bound | alone on SokHard 91/163, but 10 levels no default run solved. Portfolio (optimal, FESS, backward, X): X = weighted 116/119 vs X = greedy 127/126 (two interleaved runs); keeping weighted and dropping optimal backward instead: 120/119. All packs: Sasquatch 29 → 32, Microban IV 100 → 98 (#60, #96 need w = 3: greedy, w = 5 and w = 10 all fail them), others equal. Greedy forward alone 47 (2 unique), weighted forward w = 3 alone 59 (none) | kept: default is optimal, FESS, backward, backward-greedy |
| Greedy search expands each position once (a re-reached position is a duplicate even with a smaller g) | greedy ordering ignores g, so re-expanding on a cheaper path only repeats work; completeness is unaffected | SokHard 122/124 → 146/147 (two interleaved runs), Microban IV 98 → 100, Sasquatch 32 = 32, all other packs equal, no soundness warnings | kept (2 lines) |
| Backward TT key as one slice with reused materialization buffers (no allocation per pop) | 60–70% of pops are duplicates | identical node counts at a fixed 400k-node budget, times within noise (SokHard 63/70/100) | rejected: allocation is not the bottleneck |
| Expand-once for weighted backward (w = 3) as the partner of greedy, replacing optimal backward | the same reasoning as greedy expand-once | SokHard 147/146 vs 146/144, Microban IV 100 vs 99, Sasquatch and Microban II equal | rejected: within noise |
| Forward greedy / weighted (w = 3) thread with expand-once | stronger alone than with reopening (SokHard 81 vs 47) | levels the default misses: SokHard 2 (#91, #109) and 1; none on XSokoban, Holland or Sasquatch | rejected: no portfolio value |
| Crystalline goals (Festival `fix_crystaline`): goals whose box the solved position can never release, under a relaxation that frees at least as much as any real solution, are fixed walls; empty → dead. Iterated with frozen-on-goal boxes, cached per frozen set, used by A\* and FESS | sound (replay test on Microban solutions; identical optimal pushes on all Microban sets) | XSokoban 67 = 67, Microban, Sasquatch, Grigr2001 equal; Holland 62 vs 61 and SokHard 148 vs 146, but every gained level was solved by greedy backward search at 9.3–9.7 s, which this change does not touch (noise) | rejected: no effect where it applies (branch `crystal`) |
| Transposed start-distance table in the backward search (contiguous row per square for the matching and the O(m) child bound) | child generation is ~50% of backward time | identical nodes; 5.3–7.3 s vs 5.1–7.0 s at 600k nodes (SokHard 100, 135, 83) | rejected: not the bottleneck |
| Triple dead-set check through per-pair bitsets of third squares (`third[a·n + b]` ANDed with the partner boxes' occupancy) instead of a lookup per pair of partner boxes | profiling the greedy backward search: dead-set checks were ~45% of its time, ~14 partner boxes per child (~90 triple lookups); in backward tables nearly every pair of squares is partnered, so a partner-of-partner bit test prunes nothing (measured: no change) | identical nodes and pushes on all 163 SokHard levels; 1.4–1.8× faster at 600k nodes; portfolio SokHard 146/146 → 157/153, Microban IV 99 → 100, Sasquatch, Holland, XSokoban, Grigr2001 equal | kept (~25 lines, 230 KB per table) |
| Larger corral mini-search limits (Festival has no box cap): 10 → 20 / 40 boxes, 400 → 1,000 / 3,000 states, budget 200k → 1M / 5M | packed goal rooms exceed the 10-box cap | FESS alone XSokoban 67 = 67 = 67 | rejected: no effect |
| "Stranded boxes" FESS feature (simplified Festival OOP): off-goal boxes that reach no empty goal with on-goal boxes as walls; cell = (packed, stranded, regions) | the failure analysis found FESS packs the easy boxes and seals the last few out | FESS alone: XSokoban 67 → 66, Holland 52 → 51, Sasquatch 26 → 25 | rejected |
| Second greedy backward thread with seeded child order (rotated box/direction generation order per node, so LIFO dives elsewhere) replacing optimal backward | greedy run times are heavy-tailed; diversity of tie order | SokHard 153/153 vs 154/155, Holland 61 vs 62, Sasquatch equal | rejected (branch `greedy-random`) |
| FESS without the relaxed (parking) plan, packing layers only | parking plans give no gradient until their first step is done (XSokoban #39, Holland #73, Sasquatch #47) | FESS alone: XSokoban 67 → 60, Sasquatch 26 → 22, Holland equal, no level gained | rejected: the plan is needed where it is used |
| Lineage-fair FESS cells: each queued move belongs to a line (the ancestor that first reached its packed count; a move raising the count starts a new line); within a cell, lines are served round-robin by service count | once one line reaches a cell its descendants flood it with weight-0 advised moves, starving a later, different line (suggested by an ideation agent from the failure analysis) | FESS alone: Holland 52 → 55, XSokoban 67 → 66 (#15: 0.8 s → timeout), Sasquatch equal; portfolio: Microban IV +2 (#57, #60), Holland #14 for #31, others equal | kept (with macro pulls below) |
| Greedy backward search over macro pulls (one box pulled any distance; pulls expanded into unit pulls only for the answer), replacing optimal backward | the move shape that made FESS strong on large levels, applied to the backward search; never built before (agent + Codex) | alone: SokHard 112 (unit pulls 152), XSokoban 34, Holland 54, Sasquatch 28, but 4 levels no default run solved (XSokoban #14, Holland #37, Sasquatch #42, SokHard #113). Portfolio: XSokoban +1, Holland +1, Sasquatch +1, SokHard equal (noise), Microban IV −1 (#99, solved in 0.6 s only by optimal backward) | kept |
| Both together vs the previous default (optimal, FESS, backward, backward-greedy) | — | XSokoban 67 → 68, Sasquatch 32 → 33, SokHard 153/154 → 155/155, Microban IV 100 → 101, Holland 62 = 62 (#14, #37 for #31, #65), others equal; no soundness warnings | kept: default is optimal, FESS, backward-greedy, backward-macro |
| Why YASS solves XSokoban misses we don't (11, 28, 31, 50, 66, 69 in 2.2 s on this machine): ablation with YASS's options | packing order off → 0/6; deadlock sets off → 6/6; its forward search with a packing order does the work (the "perimeter" setting is what enables the order) | replaying YASS's solutions and solving from prefixes: #28 needs only YASS's first push (a box parked in a niche; FESS then solves it in 1.5 s), #31/#50/#66/#69 need 210–330 of its pushes, i.e. its packing-order guidance for most of the solution (all four use parking) | diagnosis |
| Plan-guided greedy forward search (key = boxes the plan still has to place, then matching / distance from the nearest box to a next target / YASS-style g + lower bound + 32 per target with free approaching pushes), with our order, layers only, or YASS's own order for #28 | reproduce YASS's packing-order search | 0/6 on the six YASS-easy levels in every variant | rejected (branch `plan-greedy`) |
| FESS lines = first move from the start (replacing "a line restarts at every packing gain") | #28: the right first move gains nothing visible and starved behind fast-packing lines; restart-at-gain lines multiply into thousands | FESS alone XSokoban 66 → 69, Holland 55 → 53; portfolio XSokoban 67 → 70 (#25, #28, #71), Microban II +1, Holland −1 (#65, a 9.4 s greedy-backward level), Microban IV one swapped, others equal. Nesting both line kinds: XSokoban +1, Holland −2 | kept (simpler than the previous definition) |
| YASS-style packing order with parking (poc.rs: reverse game, all removable boxes per phase, one box per start square, player-return check, two parking candidates per goal, backtracking), used as a phased plan by FESS or by plan-guided greedy (completed steps + distance to the current phase's targets; also YASS's g + lower bound + 32 per target with free approaching pushes) | our relaxed plan claimed XSokoban #31/#50/#66/#69 need no parking while YASS parks on all four; YASS's "seen every start square" gate as we read it made orders fail, so it was dropped | orders now park like YASS's (#50: 8 boxes beside the goal area); but FESS reaches only 5/24 (#50), 11/26 (#66), 14/29 (#69) plan steps, and plan-greedy solves 0/6. Replaying YASS's own #50 solution through our plan: progress stalls at 7 for 170 pushes, so our phases do not match how the level is actually played | rejected for now (branch `poc`, ~250 lines); the missing piece is YASS's exact order rules and its phase search, not tried further |
| Goal-room deep dive on XSokoban #50 (YASS replay through our features) | find why FESS fails where YASS needs ~20k positions | YASS's solution: empty the room (pushes 0–110), transport boxes from the left through the room to park top-right (110–210), fill the deep goals via the bottom entrance, sealing it, then the rest from the parking. Our pruning is sound along all six YASS solutions (no push or position pruned); our consistent-filling table does credit YASS's fillings | diagnosis |
| "Misplaced boxes" FESS feature (boxes on goals not counted as packed) + clearer advisor | the room-emptying stage gains no packed count | FESS alone XSokoban 69 = 69 (one swapped); FESS never empties #50's room (min misplaced 2) | rejected |
| Leaving a goal cluster to any non-backward-dead square (parking-aware consistency) | YASS's boxes come back from parking squares | FESS alone XSokoban 69 → 66 | rejected |
| FESS with YASS's exact phased order (parked boxes feeding the last goals) and/or a plan-approach advisor | separate plan quality from plan execution | YASS's order: #66 17/29, #69 18/28, #31 15 steps, #50 5 (stuck at the first hard transport), same at 60 s; our parking plan + approach: XSokoban 69 → 67 | rejected |
| Plan executor: depth-first over plan steps, each a best-first subgoal search over macro moves (4k expansions, 3 completions per step) | commit to one transport at a time, like YASS's phases | with YASS's order: #50 reaches step 5 of 28, nothing solved on the six levels | rejected (branch `exp-combo`) |
| SokHard endgame diagnosis: lowest matching bound over time in greedy backward search | where do the remaining SokHard misses spend their time | #100 reaches h = 3 at 3.9 s and solves at 14.8 s; #154 h = 4 at 0.06 s, solves at 10 s; #83 h = 4 at 0.7 s, unsolved at 20 s. Plateau positions have almost every box on a start square; the last boxes must pass boxes already "home" | diagnosis |
| Start ball: forward breadth-first ball around the start (20k / 100k / 1M positions), the backward search stops on meeting it | the plateau looked like a short endgame | no meeting on SokHard 100, 135, 83, 154 even at 1M positions: positions with h = 3–4 are many pushes from the start | rejected (branch `start-ball`) |
| Backward packing order: packing analysis on start squares (a box leaves the start cluster by being pushed to a goal), greedy backward ranked by it lexicographically or additively (w = 2, 8) | the plateau is a packing problem in reverse | SokHard misses 0/6 in every variant (plain greedy: 2/6), 30–40% fewer nodes per second | rejected (branch `back-plan`) |
| Dual-only bounds for greedy backward search (keep feasible duals per node, lower the moved row's potential, re-solve exactly every K moves) | the exact matching at each pop was ~20% of the time | K = 4 / 16: fewer nodes per second (a full re-solve costs more than the warm start); K = 250: 27% more nodes per second but the weaker bound loses SokHard #154 and #78 | rejected (branch `dual-greedy`) |
| Packing-order search re-derived from YASS's source (agent study): child score = g − tagged moves + lower bound − 32 per filled/completed target − 2 per approached target + 2·(1 + pushed box's distance to the nearest open target); depth-first dive inside the generation loop when no worse than the parent and the open minimum; LIFO buckets with a 1-in-8 rover; phase regression; plus corral pruning, last-pushed box first, player-aware target distances | YASS solves XSokoban 11/28/31/50/66/69 in 2.6k–66k positions with exactly this | 0/5 with YASS's order or ours; each ingredient helped (#28 phase 5 reached at 15k nodes instead of 128k; #69 phase 13 of 27 at 9k nodes) but phases still stall; not ported: room and tunnel pruning, learned no-progress deadlocks, YASS's exact parking-destination bookkeeping | not kept yet (branch `posearch`) |
| Sasquatch/Holland failure analysis (agent, 13 misses) | find the lever for packs that are not time-bound | three modes: packing plan flat on transit/staging goal areas (Sasquatch 34–36: the start-square rule leaves 13–25 goals "never removable"); FESS commits to dead subtrees early (S12, S25, H64, H65; dead ancestors provable in 0.06–0.4 s, but pruning them did not help: guidance is the issue); greedy backward flat from the start or plateauing near the end (Holland #64: all boxes home at 2 s with the player sealed out) | diagnosis |
| Gated transit rule: leaving a goal cluster to any non-backward-dead square, only for clusters where the start-square rule leaves ≥ half the goals unremovable | plans for Sasquatch 34–36 become ordered (6–9 layers) | FESS alone: Sasquatch 26 → 27, Holland 52 → 53, XSokoban equal; full portfolio equal on every pack (SokHard 152 vs 155, noise) | rejected: no portfolio gain |
| Cap the relaxed-plan search at 3,000 nodes (failing searches cost ~0.8 s) | — | no gain (Holland −1 with FESS alone) | rejected |
| Greedy backward: +k to the exact bound when the player cannot reach the start player's square (requeued) | Holland #64 reaches all boxes home with the player sealed out | k = 2 solves Holland #65; portfolio: Holland +1, SokHard −2 to −5, Sasquatch −1 for k = 1–3 | rejected |

## Calibration facts

- October 2026, after greedy backward search and the faster triple check:
  all 17 SokHard levels missed at 10 s are solved within 40 s (slowest
  37.5 s, 14 of them by greedy backward search), so SokHard is purely
  time-bound. Sasquatch and Holland are not: of their 10 s misses, 0/18
  and 1/19 are solved with a 60 s limit, every other run stopping at the
  2.5 GB memory cap after 16–41 s.

- A 60 s run solves 66 of the 74 SokHard levels failed at 10 s (median
  24.5 s); 48 of those 66 are solved by the backward thread.
- A\* profile on failing SokHard levels: no dominant cost (~40k
  expansions/s): child scoring ~30%, matching ~20%, materialization ~13%,
  queue ~15%, corral ~9%.
- XSokoban failures split between early stalls (preparation needed) and
  "all but 1–3 packed" positions that are dead.
- Sasquatch is not time-bound either: a 60 s run (2.5 GB memory limit)
  solves 1 of the 21 levels failed at 10 s; most runs hit the memory limit
  after 25-50 s. Like XSokoban, it needs stronger search, not speed.
- Memory, not time, bounds long runs: on Sasquatch #5 the portfolio reaches
  2.5 GB within 30-45 s (all four threads' arenas, tables and queues). The
  backward warm-start states were first stored as 3(m+1) i64 per node
  (~900 B at 36 boxes); storing only column potentials and the assignment
  (row potentials are recomputed from complementary slackness) keeps node
  counts identical and lasts 46% longer before the memory limit.
- Unlike SokHard, XSokoban is not time-bound: a 60 s run solves only 4 of
  the 23 levels failed at 10 s (all by FESS). The gap is guidance and
  dead-end recognition, not speed.
- FESS's best positions on failed levels are genuinely dead: exhaustive
  search proves XSokoban #31's best position (16/20 packed) unsolvable in
  30 s and #14's (14/18) in 6.6 s. For #31, removing any single outside box
  leaves it dead (≈2 s proofs), so the packed room plus player position is
  the cause — a packing the relaxation-based tables accept. For #14 the
  remaining boxes cannot all get through (a capacity/routing effect).
  Remedy in principle: learned deadlock patterns (prove dead, shrink to a
  minimal dead core by removing boxes, prune positions containing it), as
  in Festival — but proofs cost 2-30 s each, too slow to pay off at 10 s.
