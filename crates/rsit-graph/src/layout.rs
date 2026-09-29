//! Port of IntelliJ `GraphLayoutBuilder` / `GraphLayoutImpl`: assigns every node a
//! layout index (a "fragment" of a branch). Branches whose heads sort first are
//! laid out to the left and own the shared history below them.

use std::cmp::Ordering;

use crate::linear::LinearGraph;

#[derive(Clone, Debug, Default)]
pub struct GraphLayout {
    layout_index: Vec<u32>,
    head_nodes: Vec<u32>,
    layout_index_for_heads: Vec<u32>,
}

impl GraphLayout {
    /// `branches` are nodes that should be treated as heads even if they have children.
    pub fn build(
        graph: &impl LinearGraph,
        branches: impl IntoIterator<Item = u32>,
        mut head_cmp: impl FnMut(u32, u32) -> Ordering,
    ) -> Self {
        // same order as Kotlin `branches + heads` (a LinkedHashSet), then a stable sort
        let mut seen = std::collections::HashSet::new();
        let mut all_heads: Vec<u32> = branches.into_iter().chain(heads(graph)).filter(|h| seen.insert(*h)).collect();
        all_heads.sort_by(|a, b| head_cmp(*a, *b));

        let mut layout_index = vec![0u32; graph.nodes_count()];
        let mut head_nodes = Vec::new();
        let mut current = 1u32;
        let mut stack: Vec<u32> = Vec::new();
        let mut down = Vec::new();

        for head in all_heads {
            if layout_index[head as usize] != 0 {
                continue;
            }
            head_nodes.push(head);
            stack.push(head);
            while let Some(&node) = stack.last() {
                let first_visit = layout_index[node as usize] == 0;
                if first_visit {
                    layout_index[node as usize] = current;
                }
                down.clear();
                graph.adjacent_edges(node, crate::EdgeFilter::NORMAL_DOWN, &mut down);
                match down.iter().filter_map(|e| e.down).find(|&c| layout_index[c as usize] == 0) {
                    Some(child) => stack.push(child),
                    None => {
                        if first_visit {
                            current += 1;
                        }
                        stack.pop();
                    }
                }
            }
        }

        let layout_index_for_heads = head_nodes.iter().map(|&h| layout_index[h as usize]).collect();
        Self { layout_index, head_nodes, layout_index_for_heads }
    }

    pub fn layout_index(&self, node: u32) -> u32 {
        self.layout_index[node as usize]
    }

    pub fn head_nodes(&self) -> &[u32] {
        &self.head_nodes
    }

    /// Head of the branch whose fragment contains `node`.
    pub fn one_of_head_nodes(&self, node: u32) -> u32 {
        let li = self.layout_index(node);
        let order = match self.layout_index_for_heads.binary_search(&li) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        self.head_nodes[order]
    }
}

/// Nodes without children.
pub fn heads(graph: &impl LinearGraph) -> Vec<u32> {
    let mut up = Vec::new();
    (0..graph.nodes_count() as u32)
        .filter(|&i| {
            up.clear();
            graph.adjacent_edges(i, crate::EdgeFilter::NORMAL_UP, &mut up);
            up.is_empty()
        })
        .collect()
}
