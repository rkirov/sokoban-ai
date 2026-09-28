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
| Microban I–IV | 480 | 486 |
| SokHard | 61 (first measured) | 98 |
| Holland / Sasquatch / Grigr2001 / SokEvo | 57 / 26 / 90 / 107 | 59 / 29 / 93 / 107 |

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
| Weighted A\* thread | no unique Microban solves | opt-in only |
| Backward (pull) A\* replacing the second FESS thread | SokHard +3, Microban +1 | kept |
| Bidir expands the smaller frontier (Pohl) instead of alternating | SokHard +6 | kept |
| Weighted backward thread (w = 3) | alone 70 vs 61, portfolio 97 → 87 | rejected: solo strength ≠ portfolio value |
| Bidir forward side with A\*'s gate and frozen-wall rules | bidir alone 77 → 84, portfolio neutral | reverted |
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
| Warm-start matching from the parent's saved state (backward) | 17–25% faster; portfolio 94 vs 97 | dropped (patch kept) |
| Warm-start matching (forward A\*) | not measured once the backward result was known | dropped |

## Ideas from Codex (asked for novel first-principles ideas)

| Idea | Assessment | Status |
|------|------------|--------|
| Seed backward dead-set tables only from the actual start-player region | sound (projection of a real solution) and free | backward A\* with a fixed 400k-node budget on SokHard: 47 → 48, identical optimal pushes; wall-clock portfolio too noisy to resolve | kept (7 lines) |
| Dual-based O(m) child lower bounds, exact matching only when popped | sound (weak duality) | not tried yet |
| Backward "freeze": erase pullable boxes to a fixed point; leftovers never move | sound | not tried yet |
| Generation-time duplicate rejection in backward search | plausible | not tried |
| Player-side-aware single-box distances | known technique | not tried |
| FESS advisors for reopening goals / two-move preparation witnesses | speculative | not tried |

## Calibration facts

- A 60 s run solves 66 of the 74 SokHard levels failed at 10 s (median
  24.5 s); 48 of those 66 are solved by the backward thread.
- A\* profile on failing SokHard levels: no dominant cost (~40k
  expansions/s): child scoring ~30%, matching ~20%, materialization ~13%,
  queue ~15%, corral ~9%.
- XSokoban failures split between early stalls (preparation needed) and
  "all but 1–3 packed" positions that are dead.
