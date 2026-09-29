//! Minimum-cost bipartite matching (Hungarian algorithm, Jonker-Volgenant
//! style with potentials). Used as the admissible lower bound: boxes are
//! matched to goals with relaxed push distances as costs.
//!
//! `Matcher` supports the solver's hot path: one full solve per expanded
//! node, then for each generated child (one box moved one square, i.e. one
//! cost row changed) a single-row re-augmentation from the node's snapshot
//! instead of a from-scratch solve (dynamic Hungarian, Mills-Tettey &
//! Stentz). Rectangular problems (n boxes <= m goals) are padded internally
//! with zero-cost dummy rows to keep the matching perfect and square: after
//! unassigning the changed row exactly one column is free, so the
//! re-augmentation has a forced terminal and rectangular complementary
//! slackness never comes into play.

const BIG: i64 = 1 << 40;

/// One-shot matching; `None` when no perfect matching of rows exists (in the
/// solver: some box can never reach any goal left for it — a deadlock).
pub fn min_cost_matching(n: usize, m: usize, cost: impl Fn(usize, usize) -> Option<u32>) -> Option<u64> {
    let mut matcher = Matcher::new();
    matcher.solve(n, m, cost)
}

pub struct Matcher {
    /// Real row count (boxes); rows n..m are zero-cost dummies.
    n: usize,
    m: usize,
    /// Row and column potentials, 1-based; p[j] = row matched to column j
    /// (0 = unmatched, column 0 is the virtual start column).
    u: Vec<i64>,
    v: Vec<i64>,
    p: Vec<usize>,
    way: Vec<usize>,
    minv: Vec<i64>,
    used: Vec<bool>,
    // Snapshot of (u, v, p) for restore() between children.
    su: Vec<i64>,
    sv: Vec<i64>,
    sp: Vec<usize>,
}

impl Matcher {
    pub fn new() -> Self {
        Matcher {
            n: 0,
            m: 0,
            u: Vec::new(),
            v: Vec::new(),
            p: Vec::new(),
            way: Vec::new(),
            minv: Vec::new(),
            used: Vec::new(),
            su: Vec::new(),
            sv: Vec::new(),
            sp: Vec::new(),
        }
    }

    /// Padded cost: rows beyond the real n are zero-cost dummies that soak up
    /// the surplus columns, making the rectangular problem square without
    /// changing the optimum over real rows. 1-based indices.
    fn padded(n: usize, cost: &impl Fn(usize, usize) -> Option<u32>, i: usize, j: usize) -> i64 {
        if i <= n {
            cost(i - 1, j - 1).map_or(BIG, |c| c as i64)
        } else {
            0
        }
    }

    /// Full solve for an n x m matrix (n <= m). Leaves internal state ready
    /// for `snapshot`/`resolve_row`.
    pub fn solve(&mut self, n: usize, m: usize, cost: impl Fn(usize, usize) -> Option<u32>) -> Option<u64> {
        debug_assert!(n <= m);
        self.n = n;
        self.m = m;
        self.u.clear();
        self.u.resize(m + 1, 0);
        self.v.clear();
        self.v.resize(m + 1, 0);
        self.p.clear();
        self.p.resize(m + 1, 0);
        self.way.clear();
        self.way.resize(m + 1, 0);
        self.minv.resize(m + 1, 0);
        self.used.resize(m + 1, false);
        if m == 0 {
            return Some(0);
        }
        for i in 1..=m {
            self.insert_row(i, &cost);
        }
        self.total(&cost)
    }

    /// Append the solved state to `v_out` / `p_out` (m + 1 values each), so
    /// a child position (one row changed) can warm-start from it with `load`
    /// + `resolve_row` in O(m^2) instead of a full O(m^3) solve. Row
    /// potentials are not stored: every row is matched in the optimum and
    /// u_i + v_j = c_ij holds on matched pairs, so `load` recomputes them.
    pub fn save(&self, v_out: &mut Vec<i64>, p_out: &mut Vec<u16>) {
        v_out.extend_from_slice(&self.v);
        p_out.extend(self.p.iter().map(|&x| x as u16));
    }

    /// Restore a state written by `save` for an n x m problem with the
    /// same costs it was solved with (`cost`).
    pub fn load(&mut self, n: usize, m: usize, v: &[i64], p: &[u16], cost: impl Fn(usize, usize) -> Option<u32>) {
        let k = m + 1;
        self.n = n;
        self.m = m;
        self.v.clear();
        self.v.extend_from_slice(v);
        self.p.clear();
        self.p.extend(p.iter().map(|&x| x as usize));
        self.u.clear();
        self.u.resize(k, 0);
        for j in 1..k {
            let i = self.p[j];
            if i != 0 {
                self.u[i] = Self::padded(n, &cost, i, j) - self.v[j];
            }
        }
        self.way.resize(k, 0);
        self.minv.resize(k, 0);
        self.used.resize(k, false);
    }

    /// Admissible lower bound on the optimum after row `r0`'s costs change,
    /// in O(m), from the current optimal state (value `total`): replacing
    /// u_r by the row's minimum reduced cost min_j (c'_rj - v_j) keeps the
    /// duals feasible, and any feasible dual objective (sum u + sum v) is a
    /// lower bound on the new optimum (weak duality). None if the row has no
    /// finite cost (that box can reach no column at all).
    pub fn row_lower_bound(&self, r0: usize, total: u64, cost: impl Fn(usize, usize) -> Option<u32>) -> Option<u64> {
        let r = r0 + 1;
        let mut best = i64::MAX;
        for j in 1..=self.m {
            if let Some(c) = cost(r0, j - 1) {
                best = best.min(c as i64 - self.v[j]);
            }
        }
        (best != i64::MAX).then(|| (total as i64 - self.u[r] + best).max(0) as u64)
    }

    /// Save (u, v, p) so children can each re-augment from the node's state.
    pub fn snapshot(&mut self) {
        self.su.clone_from(&self.u);
        self.sv.clone_from(&self.v);
        self.sp.clone_from(&self.p);
    }

    pub fn restore(&mut self) {
        self.u.clone_from(&self.su);
        self.v.clone_from(&self.sv);
        self.p.clone_from(&self.sp);
    }

    /// Re-optimize after the costs of real row `r0` (0-based) changed,
    /// starting from a solved state for the old matrix. All other rows'
    /// costs must be unchanged. One augmentation stage, O(m^2) worst case.
    pub fn resolve_row(&mut self, r0: usize, cost: impl Fn(usize, usize) -> Option<u32>) -> Option<u64> {
        let r = r0 + 1;
        debug_assert!(r <= self.n);
        // Unassign row r: exactly one column becomes free and must be the
        // terminal of the coming augmentation. Other rows' duals stay
        // feasible and tight since their costs are unchanged.
        for j in 1..=self.m {
            if self.p[j] == r {
                self.p[j] = 0;
                break;
            }
        }
        // Restore dual feasibility for row r under its new costs.
        let mut ur = i64::MAX;
        for j in 1..=self.m {
            ur = ur.min(Self::padded(self.n, &cost, r, j) - self.v[j]);
        }
        self.u[r] = ur;
        self.insert_row(r, &cost);
        self.total(&cost)
    }

    /// The standard JV row-insertion (shortest augmenting path with duals).
    fn insert_row(&mut self, i: usize, cost: &impl Fn(usize, usize) -> Option<u32>) {
        self.p[0] = i;
        let mut j0 = 0usize;
        for j in 0..=self.m {
            self.minv[j] = i64::MAX;
            self.used[j] = false;
        }
        loop {
            self.used[j0] = true;
            let i0 = self.p[j0];
            let mut delta = i64::MAX;
            let mut j1 = 0usize;
            for j in 1..=self.m {
                if self.used[j] {
                    continue;
                }
                let cur = Self::padded(self.n, cost, i0, j) - self.u[i0] - self.v[j];
                if cur < self.minv[j] {
                    self.minv[j] = cur;
                    self.way[j] = j0;
                }
                if self.minv[j] < delta {
                    delta = self.minv[j];
                    j1 = j;
                }
            }
            for j in 0..=self.m {
                if self.used[j] {
                    self.u[self.p[j]] += delta;
                    self.v[j] -= delta;
                } else {
                    self.minv[j] -= delta;
                }
            }
            j0 = j1;
            if self.p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = self.way[j0];
            self.p[j0] = self.p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }

    fn total(&self, cost: &impl Fn(usize, usize) -> Option<u32>) -> Option<u64> {
        let mut total: u64 = 0;
        for j in 1..=self.m {
            let i = self.p[j];
            if i != 0 && i <= self.n {
                match cost(i - 1, j - 1) {
                    Some(c) => total += c as u64,
                    None => return None, // forced through an impossible pairing
                }
            }
        }
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_matrix() {
        let costs = [[4u32, 1, 3], [2, 0, 5], [3, 2, 2]];
        let total = min_cost_matching(3, 3, |i, j| Some(costs[i][j])).unwrap();
        assert_eq!(total, 5); // 1 + 2 + 2
    }

    #[test]
    fn rectangular() {
        let costs = [[10u32, 1, 10, 10], [10, 10, 1, 10]];
        let total = min_cost_matching(2, 4, |i, j| Some(costs[i][j])).unwrap();
        assert_eq!(total, 2);
    }

    #[test]
    fn infeasible() {
        // Row 1 has no valid column.
        let total = min_cost_matching(2, 2, |i, _| if i == 1 { None } else { Some(1) });
        assert_eq!(total, None);
    }

    #[test]
    fn forced_expensive() {
        // Both rows prefer column 0; one must take its only alternative.
        let costs = [[1u32, 100], [1, 50]];
        let total = min_cost_matching(2, 2, |i, j| Some(costs[i][j])).unwrap();
        assert_eq!(total, 51);
    }

    /// resolve_row must agree with a from-scratch solve for every single-row
    /// perturbation of pseudo-random matrices of various shapes.
    #[test]
    fn resolve_row_matches_full_solve() {
        for (n, m, seed) in [(6, 8, 0usize), (5, 5, 1), (8, 8, 2), (3, 10, 3), (10, 12, 4)] {
            // Deterministic pseudo-random costs.
            let base: Vec<Vec<u32>> = (0..n)
                .map(|i| {
                    (0..m)
                        .map(|j| ((i * 31 + j * 17 + seed * 41 + (i * j + seed) % 13) % 29) as u32)
                        .collect()
                })
                .collect();
            for r in 0..n {
                for delta in [-7i64, -2, -1, 1, 3, 9] {
                    // Perturb entries unevenly so relative order shifts too.
                    let perturbed: Vec<u32> = (0..m)
                        .map(|j| (base[r][j] as i64 + delta * ((j % 3) as i64)).max(0) as u32)
                        .collect();
                    let cost_new = |i: usize, j: usize| -> Option<u32> {
                        Some(if i == r { perturbed[j] } else { base[i][j] })
                    };
                    let mut matcher = Matcher::new();
                    matcher.solve(n, m, |i, j| Some(base[i][j])).unwrap();
                    matcher.snapshot();
                    // Twice from the same snapshot: restore must be complete.
                    for round in 0..2 {
                        matcher.restore();
                        let incremental = matcher.resolve_row(r, cost_new).unwrap();
                        let full = min_cost_matching(n, m, cost_new).unwrap();
                        assert_eq!(
                            incremental, full,
                            "n {n} m {m} seed {seed} row {r} delta {delta} round {round}"
                        );
                    }
                }
            }
        }
    }

    /// resolve_row must detect infeasibility introduced by the new row.
    #[test]
    fn resolve_row_detects_infeasible() {
        let costs = [[1u32, 2], [3, 4]];
        let mut matcher = Matcher::new();
        matcher.solve(2, 2, |i, j| Some(costs[i][j])).unwrap();
        let out = matcher.resolve_row(0, |i, j| if i == 0 { None } else { Some(costs[i][j]) });
        assert_eq!(out, None);
    }
}
