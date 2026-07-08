//! Patch graph data model and policy edge generators (v1.1.0).
//!
//! A *policy* is a rule for which old→new patch edges exist. Pairwise
//! diffs are not one strategy: adjacent-only, sparse dyadic ladders,
//! base-version hubs, hot pairs and all-pairs are different graphs with
//! different storage/steps/bytes tradeoffs. The all-pairs graph is kept
//! only as the theoretical one-hop baseline — it is not how pairwise
//! systems are normally deployed.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One build in the ordered version stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionNode {
    pub id: String,
    pub index: usize,
    pub path: String,
    pub size_bytes: u64,
    /// Full build compressed with zstd-3 — the "full download" fallback
    /// and reinstall cost for pairwise policies. 0 in structure-only graphs.
    pub compressed_bytes: u64,
    /// blake3 over content (file) or over the sorted (path,size) walk (dir).
    pub signature_hash: String,
}

/// CAVS route data embedded in the graph so `patch-policy simulate` can
/// replay traffic without the original builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CavsRoutes {
    pub store_bytes: u64,
    pub build_ms: u64,
    /// cold[from][to]: cold cache + previous install (the default state).
    pub cold: Vec<Vec<u64>>,
    /// warm[from][to]: cache accumulated across every version ≤ from.
    pub warm: Vec<Vec<u64>>,
    /// install[to]: full (re)install from the store.
    pub install: Vec<u64>,
}

/// One engine's measurement of one edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeMeasure {
    pub engine: String,
    pub raw_patch_bytes: u64,
    pub compressed_patch_bytes: u64,
    pub diff_ms: u64,
    pub apply_ms: u64,
    pub verify_ms: u64,
    pub peak_rss_mib: Option<f64>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEdge {
    pub from: usize,
    pub to: usize,
    /// Which policies use this edge (an edge can belong to several).
    pub policies: Vec<String>,
    pub measures: Vec<EdgeMeasure>,
}

impl PatchEdge {
    pub fn bytes(&self, engine: &str) -> Option<u64> {
        self.measures
            .iter()
            .find(|m| m.engine == engine && m.verified)
            .map(|m| m.compressed_patch_bytes)
    }
    pub fn measure(&self, engine: &str) -> Option<&EdgeMeasure> {
        self.measures.iter().find(|m| m.engine == engine)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySpec {
    pub name: String,
    /// Human label used in reports; all-pairs is always labeled as the
    /// theoretical one-hop baseline, never as "pairwise diffs".
    pub label: String,
    pub edge_idxs: Vec<usize>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchGraph {
    pub versions: Vec<VersionNode>,
    pub edges: Vec<PatchEdge>,
    pub policies: Vec<PolicySpec>,
    /// CAVS chunk-route data; None in structure-only graphs.
    pub cavs_routes: Option<CavsRoutes>,
    /// True when edges were generated but not measured (`patch-policy graph`).
    pub structure_only: bool,
    pub tool_versions: BTreeMap<String, String>,
}

impl PatchGraph {
    pub fn policy(&self, name: &str) -> Option<&PolicySpec> {
        self.policies.iter().find(|p| p.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderMode {
    /// Aligned dyadic intervals: level d edges start at multiples of d
    /// (v0→v1…, v0→v2, v2→v4…, v0→v4…). Fewer than 2N patches. Default.
    Aligned,
    /// Every start offset per level (v1→v3 as well as v0→v2). More edges
    /// (~N·log N), shorter chains for unaligned jumps.
    Dense,
}

impl LadderMode {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "aligned" => Ok(LadderMode::Aligned),
            "dense" => Ok(LadderMode::Dense),
            other => bail!("unknown --ladder-mode {other:?} (aligned, dense)"),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LadderMode::Aligned => "aligned",
            LadderMode::Dense => "dense",
        }
    }
}

/// v0→v1, v1→v2, … — O(N) storage, chains for skips.
pub fn adjacent_pairs(n: usize) -> Vec<(usize, usize)> {
    (0..n.saturating_sub(1)).map(|i| (i, i + 1)).collect()
}

/// Sparse power-of-two ladder over dyadic intervals.
pub fn ladder_pairs(n: usize, mode: LadderMode) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut d = 1usize;
    while n > 0 && d < n {
        match mode {
            LadderMode::Aligned => {
                let mut start = 0;
                while start + d < n {
                    out.push((start, start + d));
                    start += d;
                }
            }
            LadderMode::Dense => {
                for start in 0..n - d {
                    out.push((start, start + d));
                }
            }
        }
        d *= 2;
    }
    out.sort();
    out.dedup();
    out
}

/// Base/hub edges: base→vi always; vi→base when bidirectional (needed to
/// route arbitrary old→new jumps through the hub).
pub fn base_pairs(n: usize, base: usize, bidirectional: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for i in 0..n {
        if i == base {
            continue;
        }
        out.push((base, i));
        if bidirectional {
            out.push((i, base));
        }
    }
    out
}

/// Adjacent baseline plus direct old→latest edges for the K most recent
/// old versions (k=1 is already the adjacent edge).
pub fn hot_pairs_latest_k(n: usize, k: usize) -> Vec<(usize, usize)> {
    let mut out = adjacent_pairs(n);
    if n >= 2 {
        let latest = n - 1;
        for back in 2..=k {
            if back >= n {
                break;
            }
            out.push((latest - back, latest));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every i<j pair — the O(N²) theoretical one-hop baseline.
pub fn all_pairs(n: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            out.push((i, j));
        }
    }
    out
}

/// Cheapest path from `from` to `to` using only the policy's edges.
/// Cost is (total bytes, steps) lexicographic; with `bytes_of` returning
/// None for every edge (structure-only graphs) it degrades to fewest
/// steps. Returns the edge indexes along the path, or None if unreachable.
pub fn cheapest_path(
    edges: &[PatchEdge],
    edge_idxs: &[usize],
    n: usize,
    from: usize,
    to: usize,
    bytes_of: &dyn Fn(&PatchEdge) -> Option<u64>,
) -> Option<Vec<usize>> {
    if from == to {
        return Some(Vec::new());
    }
    // adjacency: node -> (next, edge_idx, bytes)
    let mut adj: Vec<Vec<(usize, usize, u64)>> = vec![Vec::new(); n];
    for &e in edge_idxs {
        let edge = &edges[e];
        let bytes = bytes_of(edge).unwrap_or(1); // structure-only: hop count
        adj[edge.from].push((edge.to, e, bytes));
    }
    // Dijkstra over (bytes, steps).
    let mut best: Vec<(u64, u64)> = vec![(u64::MAX, u64::MAX); n];
    let mut prev: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut heap = std::collections::BinaryHeap::new();
    best[from] = (0, 0);
    heap.push(std::cmp::Reverse((0u64, 0u64, from)));
    while let Some(std::cmp::Reverse((bytes, steps, node))) = heap.pop() {
        if (bytes, steps) > best[node] {
            continue;
        }
        if node == to {
            break;
        }
        for &(next, e, w) in &adj[node] {
            let cand = (bytes.saturating_add(w), steps + 1);
            if cand < best[next] {
                best[next] = cand;
                prev[next] = Some((node, e));
                heap.push(std::cmp::Reverse((cand.0, cand.1, next)));
            }
        }
    }
    if best[to].0 == u64::MAX {
        return None;
    }
    let mut path = Vec::new();
    let mut cursor = to;
    while cursor != from {
        let (p, e) = prev[cursor]?;
        path.push(e);
        cursor = p;
    }
    path.reverse();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: usize, to: usize, bytes: u64) -> PatchEdge {
        PatchEdge {
            from,
            to,
            policies: vec!["test".into()],
            measures: vec![EdgeMeasure {
                engine: "test".into(),
                raw_patch_bytes: bytes,
                compressed_patch_bytes: bytes,
                diff_ms: 0,
                apply_ms: 0,
                verify_ms: 0,
                peak_rss_mib: None,
                verified: true,
            }],
        }
    }

    #[test]
    fn adjacent_is_linear() {
        assert_eq!(adjacent_pairs(5), vec![(0, 1), (1, 2), (2, 3), (3, 4)]);
        assert!(adjacent_pairs(1).is_empty());
    }

    #[test]
    fn aligned_ladder_stays_under_2n() {
        for n in [2usize, 5, 9, 16, 33, 100] {
            let edges = ladder_pairs(n, LadderMode::Aligned);
            assert!(edges.len() < 2 * n, "n={n} produced {} edges", edges.len());
            // level-1 edges are the full adjacent chain
            for i in 0..n - 1 {
                assert!(edges.contains(&(i, i + 1)));
            }
        }
        // v0..v8 example from the spec
        let e = ladder_pairs(9, LadderMode::Aligned);
        for pair in [(0, 2), (2, 4), (4, 6), (6, 8), (0, 4), (4, 8), (0, 8)] {
            assert!(e.contains(&pair), "missing {pair:?}");
        }
    }

    #[test]
    fn dense_ladder_covers_unaligned_starts() {
        let e = ladder_pairs(6, LadderMode::Dense);
        assert!(e.contains(&(1, 3)));
        assert!(e.contains(&(3, 5)));
        assert!(!ladder_pairs(6, LadderMode::Aligned).contains(&(1, 3)));
    }

    #[test]
    fn ladder_path_is_logarithmic() {
        let n = 33;
        let pairs = ladder_pairs(n, LadderMode::Aligned);
        let edges: Vec<PatchEdge> = pairs.iter().map(|&(f, t)| edge(f, t, 1)).collect();
        let idxs: Vec<usize> = (0..edges.len()).collect();
        let path = cheapest_path(&edges, &idxs, n, 0, n - 1, &|e| e.bytes("test")).unwrap();
        assert!(path.len() <= 2 * (n as f64).log2().ceil() as usize);
        // and an unaligned long jump still resolves
        let path = cheapest_path(&edges, &idxs, n, 1, 31, &|e| e.bytes("test")).unwrap();
        assert!(!path.is_empty() && path.len() <= 10);
    }

    #[test]
    fn base_hub_routes_need_bidirectional() {
        let n = 6;
        let one_way = base_pairs(n, 0, false);
        let edges: Vec<PatchEdge> = one_way.iter().map(|&(f, t)| edge(f, t, 1)).collect();
        let idxs: Vec<usize> = (0..edges.len()).collect();
        // v2→v5 impossible with base→vi only…
        assert!(cheapest_path(&edges, &idxs, n, 2, 5, &|e| e.bytes("test")).is_none());
        // …and possible through the hub with reverse edges (2 steps).
        let both = base_pairs(n, 0, true);
        let edges: Vec<PatchEdge> = both.iter().map(|&(f, t)| edge(f, t, 1)).collect();
        let idxs: Vec<usize> = (0..edges.len()).collect();
        let path = cheapest_path(&edges, &idxs, n, 2, 5, &|e| e.bytes("test")).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(both.len(), 2 * (n - 1));
    }

    #[test]
    fn all_pairs_is_quadratic_and_one_hop() {
        let n = 10;
        let pairs = all_pairs(n);
        assert_eq!(pairs.len(), n * (n - 1) / 2);
        let edges: Vec<PatchEdge> = pairs.iter().map(|&(f, t)| edge(f, t, 1)).collect();
        let idxs: Vec<usize> = (0..edges.len()).collect();
        let path = cheapest_path(&edges, &idxs, n, 3, 9, &|e| e.bytes("test")).unwrap();
        assert_eq!(path.len(), 1);
    }

    #[test]
    fn hot_pairs_add_direct_edges_to_latest() {
        let pairs = hot_pairs_latest_k(10, 3);
        assert!(pairs.contains(&(7, 9)) && pairs.contains(&(6, 9)));
        assert!(pairs.contains(&(8, 9))); // adjacent covers k=1
        assert_eq!(pairs.len(), 9 + 2);
    }

    #[test]
    fn cheapest_path_prefers_fewer_bytes_over_fewer_steps() {
        // direct edge is expensive, two cheap hops win
        let edges = vec![edge(0, 2, 100), edge(0, 1, 10), edge(1, 2, 10)];
        let idxs = vec![0, 1, 2];
        let path = cheapest_path(&edges, &idxs, 3, 0, 2, &|e| e.bytes("test")).unwrap();
        assert_eq!(path, vec![1, 2]);
    }
}
