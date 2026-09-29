//! Commit graph model and rendering primitives.
//!
//! Port of IntelliJ `platform/vcs-log/graph` (Apache-2.0, Copyright JetBrains s.r.o.).

pub mod color;
pub mod edges_in_row;
pub mod filter;
pub mod layout;
pub mod linear;
pub mod print;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::Hash;

pub use filter::{FilteredGraph, FilteredView};
pub use layout::GraphLayout;
pub use linear::{EdgeFilter, EdgeType, GraphCommit, GraphEdge, GraphElement, LinearGraph, PermanentLinearGraph};
pub use print::{EdgeDir, PrintElement, PrintElementGenerator, PrintKind};

/// The full commit graph with its layout (port of `PermanentGraphImpl`).
pub struct PermanentGraph<Id> {
    pub linear: PermanentLinearGraph,
    pub layout: GraphLayout,
    ids: Vec<Id>,
    not_loaded: Vec<Id>,
    node_by_id: HashMap<Id, u32>,
}

impl<Id: Clone + Eq + Hash> PermanentGraph<Id> {
    /// `commits` must be topologically sorted (children before parents).
    /// `head_cmp` orders branch heads by importance; more important ones are laid
    /// out to the left and own the shared history.
    pub fn new(
        commits: &[GraphCommit<Id>],
        branch_heads: impl IntoIterator<Item = Id>,
        mut head_cmp: impl FnMut(&Id, &Id) -> Ordering,
    ) -> Self {
        let mut not_loaded = Vec::new();
        let linear = PermanentLinearGraph::build(commits, |id| {
            not_loaded.push(id.clone());
            -(not_loaded.len() as i32 + 1)
        });
        let ids: Vec<Id> = commits.iter().map(|c| c.id.clone()).collect();
        let node_by_id: HashMap<Id, u32> = ids.iter().enumerate().map(|(i, id)| (id.clone(), i as u32)).collect();
        let branch_nodes: Vec<u32> = branch_heads.into_iter().filter_map(|id| node_by_id.get(&id).copied()).collect();
        let layout = GraphLayout::build(&linear, branch_nodes, |a, b| head_cmp(&ids[a as usize], &ids[b as usize]));
        Self { linear, layout, ids, not_loaded, node_by_id }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn commit_id(&self, node: u32) -> &Id {
        &self.ids[node as usize]
    }

    /// Id of a commit referenced by a `NotLoadCommit` edge target.
    pub fn not_loaded_id(&self, target: i32) -> Option<&Id> {
        self.not_loaded.get((-target - 2) as usize)
    }

    pub fn node(&self, id: &Id) -> Option<u32> {
        self.node_by_id.get(id).copied()
    }

    pub fn parents(&self, node: u32) -> Vec<u32> {
        let mut out = Vec::new();
        self.linear.for_each_parent(node, |p| {
            if p >= 0 {
                out.push(p as u32)
            }
        });
        out
    }

    /// Nodes reachable from `starts` by parent edges (starts included).
    pub fn reachable_from(&self, starts: impl IntoIterator<Item = u32>) -> Vec<bool> {
        let mut visited = vec![false; self.len()];
        let mut stack: Vec<u32> = Vec::new();
        for s in starts {
            if (s as usize) < visited.len() && !visited[s as usize] {
                visited[s as usize] = true;
                stack.push(s);
            }
        }
        while let Some(node) = stack.pop() {
            self.linear.for_each_parent(node, |p| {
                if p >= 0 && !visited[p as usize] {
                    visited[p as usize] = true;
                    stack.push(p as u32);
                }
            });
        }
        visited
    }

    /// Nodes of `branch_heads` from which `node` is reachable
    /// (IntelliJ `ReachableNodes.getContainingBranches`).
    pub fn containing_branches(&self, node: u32, branch_heads: &std::collections::HashSet<u32>) -> Vec<u32> {
        let mut visited = vec![false; self.len()];
        let mut stack = vec![node];
        let mut result = Vec::new();
        let mut up = Vec::new();
        visited[node as usize] = true;
        while let Some(n) = stack.pop() {
            if branch_heads.contains(&n) {
                result.push(n);
            }
            up.clear();
            self.linear.adjacent_edges(n, EdgeFilter::NORMAL_UP, &mut up);
            for child in up.iter().filter_map(|e| e.up) {
                if !visited[child as usize] {
                    visited[child as usize] = true;
                    stack.push(child);
                }
            }
        }
        result.sort_unstable();
        result
    }

    pub fn children(&self, node: u32) -> Vec<u32> {
        self.linear.up_nodes(node)
    }

    /// Color id for a graph element (port of `PrintElementPresentationManagerImpl.getColorId`
    /// with `GraphColorGetterByHead`). `head_ref_name` gives the best ref name at a head commit.
    pub fn color_id<'a>(&'a self, element: &GraphElement, head_ref_name: impl Fn(&Id) -> Option<&'a str>) -> i32 {
        let node_color = |node: u32| {
            let li = self.layout.layout_index(node);
            let head = self.layout.one_of_head_nodes(node);
            let head_li = self.layout.layout_index(head);
            color::color_id(head_ref_name(&self.ids[head as usize]), head_li, li)
        };
        match element {
            GraphElement::Node(n) => node_color(*n),
            GraphElement::Edge(e) => match e.as_normal() {
                None => node_color(e.not_null_node()),
                Some((up, down)) => {
                    if self.layout.layout_index(up) >= self.layout.layout_index(down) {
                        node_color(up)
                    } else {
                        node_color(down)
                    }
                }
            },
        }
    }
}
