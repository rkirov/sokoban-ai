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
