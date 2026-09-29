//! Linear graph model. Ported from IntelliJ `LinearGraph`, `GraphEdge`,
//! `PermanentLinearGraphBuilder` and `PermanentLinearGraphImpl`.

/// Kind of an edge between two rows of the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeType {
    Usual,
    Dotted,
    /// Edge to a commit that is not part of the loaded graph.
    NotLoadCommit,
    DottedArrowUp,
    DottedArrowDown,
}

impl EdgeType {
    pub fn is_normal(self) -> bool {
        matches!(self, EdgeType::Usual | EdgeType::Dotted)
    }
}

/// An edge of a [`LinearGraph`]. Normal edges have both `up` and `down`;
/// special edges have one of them plus a `target_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphEdge {
    pub up: Option<u32>,
    pub down: Option<u32>,
    pub target_id: Option<i32>,
    pub ty: EdgeType,
}

impl GraphEdge {
    pub fn normal(up: u32, down: u32, ty: EdgeType) -> Self {
        Self { up: Some(up), down: Some(down), target_id: None, ty }
    }

    pub fn with_target(node: u32, target_id: i32, ty: EdgeType) -> Self {
        match ty {
            EdgeType::DottedArrowUp => Self { up: None, down: Some(node), target_id: Some(target_id), ty },
            _ => Self { up: Some(node), down: None, target_id: Some(target_id), ty },
        }
    }

    /// `(up, down)` for normal edges.
    pub fn as_normal(&self) -> Option<(u32, u32)> {
        if !self.ty.is_normal() {
            return None;
        }
        Some((self.up?, self.down?))
    }

    pub fn not_null_node(&self) -> u32 {
        self.up.or(self.down).expect("edge without nodes")
    }

    pub fn is_up_for(&self, node: u32) -> bool {
        self.down == Some(node)
    }

    pub fn is_down_for(&self, node: u32) -> bool {
        self.up == Some(node)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphElement {
    Node(u32),
    Edge(GraphEdge),
}

#[derive(Clone, Copy, Debug)]
pub struct EdgeFilter {
    pub special: bool,
    pub up_normal: bool,
    pub down_normal: bool,
}

impl EdgeFilter {
    pub const ALL: Self = Self { special: true, up_normal: true, down_normal: true };
    pub const NORMAL_ALL: Self = Self { special: false, up_normal: true, down_normal: true };
    pub const NORMAL_UP: Self = Self { special: false, up_normal: true, down_normal: false };
    pub const NORMAL_DOWN: Self = Self { special: false, up_normal: false, down_normal: true };
    pub const SPECIAL: Self = Self { special: true, up_normal: false, down_normal: false };
}

pub trait LinearGraph {
    fn nodes_count(&self) -> usize;

    /// Appends edges adjacent to `node` that pass `filter` to `out`.
    fn adjacent_edges(&self, node: u32, filter: EdgeFilter, out: &mut Vec<GraphEdge>);

    /// Id of the node in the permanent graph.
    fn node_id(&self, node: u32) -> i32 {
        node as i32
    }

    fn edges(&self, node: u32, filter: EdgeFilter) -> Vec<GraphEdge> {
        let mut out = Vec::new();
        self.adjacent_edges(node, filter, &mut out);
        out
    }

    fn up_nodes(&self, node: u32) -> Vec<u32> {
        self.edges(node, EdgeFilter::NORMAL_UP).iter().filter_map(|e| e.up).collect()
    }

    fn down_nodes(&self, node: u32) -> Vec<u32> {
        self.edges(node, EdgeFilter::NORMAL_DOWN).iter().filter_map(|e| e.down).collect()
    }
}

/// Input commit for building the permanent graph.
#[derive(Clone, Debug)]
pub struct GraphCommit<Id> {
    pub id: Id,
    pub parents: Vec<Id>,
}

/// Compact storage of the full commit graph: rows are commits in topological
/// order, "simple" nodes (single parent right below) need no edge storage.
#[derive(Clone, Debug, Default)]
pub struct PermanentLinearGraph {
    simple: Vec<u64>,
    len: usize,
    node_to_edge: Vec<u32>,
    /// Adjacent node per edge slot; negative values are ids of not loaded commits.
    long_edges: Vec<i32>,
}

impl PermanentLinearGraph {
    /// Builds the graph from topologically sorted commits. Parents that are not in
    /// `commits` get negative ids from `not_loaded_id` (must be `< -1`).
    pub fn build<Id: Clone + Eq + std::hash::Hash>(
        commits: &[GraphCommit<Id>],
        mut not_loaded_id: impl FnMut(&Id) -> i32,
    ) -> Self {
        let n = commits.len();
        let mut simple = vec![0u64; n.div_ceil(64)];
        let mut long_edges_count = 0usize;
        let parents_of = |i: usize| dedup_parents(&commits[i].parents);

        for i in 0..n {
            let parents = parents_of(i);
            if parents.len() == 1 && i + 1 < n && parents[0] == commits[i + 1].id {
                simple[i / 64] |= 1 << (i % 64);
            } else {
                long_edges_count += parents.len();
            }
        }
        let is_simple = |i: usize| simple[i / 64] & (1 << (i % 64)) != 0;

        let mut node_to_edge = vec![0u32; n + 1];
        let mut long_edges = vec![0i32; 2 * long_edges_count];
        // down commit id -> up node indices waiting for it
        let mut up_adjacent: std::collections::HashMap<Id, Vec<u32>> = std::collections::HashMap::new();

        for i in 0..n {
            let commit = &commits[i];
            let mut edge_index = node_to_edge[i] as usize;
            if let Some(up_nodes) = up_adjacent.remove(&commit.id) {
                for up in up_nodes {
                    // fix the underdone edge of `up`
                    let up_parents = parents_of(up as usize);
                    let end = node_to_edge[up as usize + 1] as usize;
                    let pos = up_parents.iter().position(|p| *p == commit.id).expect("underdone edge");
                    let slot = end - (up_parents.len() - pos);
                    assert_eq!(long_edges[slot], -1, "edge was set early");
                    long_edges[slot] = i as i32;
                    long_edges[edge_index] = up as i32;
                    edge_index += 1;
                }
            }
            if !is_simple(i) {
                for parent in parents_of(i).iter() {
                    up_adjacent.entry(parent.clone()).or_default().push(i as u32);
                    long_edges[edge_index] = -1;
                    edge_index += 1;
                }
            }
            node_to_edge[i + 1] = edge_index as u32;
        }

        // parents that were never loaded
        let mut missing: Vec<(Id, Vec<u32>)> = up_adjacent.into_iter().collect();
        missing.sort_by_key(|(_, ups)| *ups.iter().min().unwrap());
        for (id, ups) in missing {
            let target = not_loaded_id(&id);
            for up in ups {
                let range = node_to_edge[up as usize] as usize..node_to_edge[up as usize + 1] as usize;
                let slot = range.clone().find(|&s| long_edges[s] == -1).expect("underdone edge to not loaded commit");
                long_edges[slot] = target;
            }
        }

        long_edges.truncate(node_to_edge[n] as usize);
        long_edges.shrink_to_fit();
        Self { simple, len: n, node_to_edge, long_edges }
    }

    fn is_simple(&self, i: u32) -> bool {
        self.simple[i as usize / 64] & (1 << (i % 64)) != 0
    }

    /// Calls `f` for every parent of `node` (negative ids for not loaded commits).
    pub fn for_each_parent(&self, node: u32, mut f: impl FnMut(i32)) {
        for &adj in self.edge_slots(node) {
            if adj < 0 || (node as i32) < adj {
                f(adj);
            }
        }
        if self.is_simple(node) {
            f(node as i32 + 1);
        }
    }

    fn edge_slots(&self, node: u32) -> &[i32] {
        &self.long_edges[self.node_to_edge[node as usize] as usize..self.node_to_edge[node as usize + 1] as usize]
    }
}

impl LinearGraph for PermanentLinearGraph {
    fn nodes_count(&self) -> usize {
        self.len
    }

    fn adjacent_edges(&self, node: u32, filter: EdgeFilter, out: &mut Vec<GraphEdge>) {
        if node != 0 && self.is_simple(node - 1) && filter.up_normal {
            out.push(GraphEdge::normal(node - 1, node, EdgeType::Usual));
        }
        for &adj in self.edge_slots(node) {
            if adj < 0 {
                if filter.special {
                    out.push(GraphEdge::with_target(node, adj, EdgeType::NotLoadCommit));
                }
                continue;
            }
            let adj = adj as u32;
            if node > adj && filter.up_normal {
                out.push(GraphEdge::normal(adj, node, EdgeType::Usual));
            }
            if node < adj && filter.down_normal {
                out.push(GraphEdge::normal(node, adj, EdgeType::Usual));
            }
        }
        if self.is_simple(node) && filter.down_normal {
            out.push(GraphEdge::normal(node, node + 1, EdgeType::Usual));
        }
    }
}

/// Port of `DuplicateParentFixer`: a commit may list the same parent twice.
fn dedup_parents<Id: Clone + Eq>(parents: &[Id]) -> std::borrow::Cow<'_, [Id]> {
    let has_dups = parents.iter().enumerate().any(|(i, p)| parents[..i].contains(p));
    if !has_dups {
        return std::borrow::Cow::Borrowed(parents);
    }
    let mut out: Vec<Id> = Vec::with_capacity(parents.len());
    for p in parents {
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    std::borrow::Cow::Owned(out)
}
