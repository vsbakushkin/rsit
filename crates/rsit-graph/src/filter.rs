//! Filtered view of a graph: only some nodes are visible and hidden history is
//! bridged by dotted edges. Port of IntelliJ `CollapsedGraph` (read-only part),
//! `EdgeStorage` and `DottedFilterEdgesGenerator`.

use std::collections::HashMap;

use crate::linear::{EdgeFilter, EdgeType, GraphEdge, LinearGraph};

/// An extra edge stored at a delegate node: the other end (delegate index) for
/// normal edges, `None` for arrows.
type ExtraEdge = (Option<u32>, EdgeType);

pub struct FilteredGraph {
    /// Visible row -> delegate row.
    to_delegate: Vec<u32>,
    /// Delegate row -> visible row, `u32::MAX` when hidden.
    from_delegate: Vec<u32>,
    extra: HashMap<u32, Vec<ExtraEdge>>,
}

impl FilteredGraph {
    /// `visible[i]` tells whether delegate node `i` stays visible.
    pub fn new(delegate: &impl LinearGraph, visible: &[bool]) -> Self {
        let n = delegate.nodes_count();
        assert_eq!(visible.len(), n);
        let mut to_delegate = Vec::new();
        let mut from_delegate = vec![u32::MAX; n];
        for (i, &v) in visible.iter().enumerate() {
            if v {
                from_delegate[i] = to_delegate.len() as u32;
                to_delegate.push(i as u32);
            }
        }
        let mut this = Self { to_delegate, from_delegate, extra: HashMap::new() };
        this.generate_dotted_edges(delegate, visible);
        this
    }

    pub fn len(&self) -> usize {
        self.to_delegate.len()
    }

    pub fn is_empty(&self) -> bool {
        self.to_delegate.is_empty()
    }

    pub fn to_delegate(&self, row: u32) -> u32 {
        self.to_delegate[row as usize]
    }

    pub fn from_delegate(&self, delegate_row: u32) -> Option<u32> {
        match self.from_delegate.get(delegate_row as usize) {
            Some(&v) if v != u32::MAX => Some(v),
            _ => None,
        }
    }

    pub fn view<'a, G: LinearGraph>(&'a self, delegate: &'a G) -> FilteredView<'a, G> {
        FilteredView { filter: self, delegate }
    }

    fn add_dotted_edge(&mut self, a: u32, b: u32) {
        let (up, down) = if a < b { (a, b) } else { (b, a) };
        let edge = (Some(down), EdgeType::Dotted);
        let list = self.extra.entry(up).or_default();
        if list.contains(&edge) {
            return;
        }
        list.push(edge);
        self.extra.entry(down).or_default().push((Some(up), EdgeType::Dotted));
    }

    /// `DottedFilterEdgesGenerator.update` over the whole graph.
    fn generate_dotted_edges(&mut self, graph: &impl LinearGraph, visible: &[bool]) {
        let n = graph.nodes_count();
        let mut numbers = vec![0i32; n];
        let mut buf = Vec::new();

        // down walk
        for current in 0..n {
            buf.clear();
            graph.adjacent_edges(current as u32, EdgeFilter::NORMAL_UP, &mut buf);
            if visible[current] {
                let mut nearly_up = i32::MIN;
                let mut max_adj = i32::MIN;
                for up in buf.iter().filter_map(|e| e.up) {
                    if visible[up as usize] {
                        max_adj = max_adj.max(numbers[up as usize]);
                    } else {
                        nearly_up = nearly_up.max(numbers[up as usize]);
                    }
                }
                if nearly_up == max_adj || nearly_up == i32::MIN {
                    numbers[current] = max_adj;
                } else {
                    self.add_dotted_edge(current as u32, nearly_up as u32);
                    numbers[current] = nearly_up;
                }
            } else {
                let mut nearly_up = i32::MIN;
                for up in buf.iter().filter_map(|e| e.up) {
                    let candidate = if visible[up as usize] { up as i32 } else { numbers[up as usize] };
                    nearly_up = nearly_up.max(candidate);
                }
                numbers[current] = nearly_up;
            }
        }

        numbers.fill(i32::MAX);

        // up walk
        for current in (0..n).rev() {
            buf.clear();
            graph.adjacent_edges(current as u32, EdgeFilter::NORMAL_DOWN, &mut buf);
            if visible[current] {
                let mut nearly_down = i32::MAX;
                let mut min_adj = i32::MAX;
                for down in buf.iter().filter_map(|e| e.down) {
                    if visible[down as usize] {
                        min_adj = min_adj.min(numbers[down as usize]);
                    } else {
                        nearly_down = nearly_down.min(numbers[down as usize]);
                    }
                }
                if nearly_down == min_adj || nearly_down == i32::MAX {
                    numbers[current] = min_adj;
                } else {
                    self.add_dotted_edge(current as u32, nearly_down as u32);
                    numbers[current] = nearly_down;
                }
            } else {
                let mut nearly_down = i32::MAX;
                for down in buf.iter().filter_map(|e| e.down) {
                    let candidate = if visible[down as usize] { down as i32 } else { numbers[down as usize] };
                    nearly_down = nearly_down.min(candidate);
                }
                numbers[current] = nearly_down;
            }
        }
    }
}

/// The filtered graph as a [`LinearGraph`] over visible rows
/// (IntelliJ `CollapsedGraph.CompiledGraph`).
pub struct FilteredView<'a, G> {
    filter: &'a FilteredGraph,
    delegate: &'a G,
}

impl<G: LinearGraph> LinearGraph for FilteredView<'_, G> {
    fn nodes_count(&self) -> usize {
        self.filter.len()
    }

    fn node_id(&self, node: u32) -> i32 {
        self.delegate.node_id(self.filter.to_delegate(node))
    }

    fn adjacent_edges(&self, node: u32, filter: EdgeFilter, out: &mut Vec<GraphEdge>) {
        let delegate_index = self.filter.to_delegate(node);
        let mut delegate_edges = Vec::new();
        self.delegate.adjacent_edges(delegate_index, filter, &mut delegate_edges);
        let compiled = |i: Option<u32>| -> Result<Option<u32>, ()> {
            match i {
                None => Ok(None),
                Some(i) => self.filter.from_delegate(i).map(Some).ok_or(()),
            }
        };
        for e in delegate_edges {
            if let (Ok(up), Ok(down)) = (compiled(e.up), compiled(e.down)) {
                out.push(GraphEdge { up, down, ..e });
            }
        }
        let Some(extra) = self.filter.extra.get(&delegate_index) else {
            return;
        };
        for &(other, ty) in extra {
            if ty.is_normal() {
                let Some(other) = other.and_then(|o| self.filter.from_delegate(o)) else {
                    continue;
                };
                let edge = GraphEdge::normal(node.min(other), node.max(other), ty);
                let pass = if edge.down == Some(node) { filter.up_normal } else { filter.down_normal };
                if pass {
                    out.push(edge);
                }
            } else if filter.special {
                out.push(GraphEdge::with_target(node, other.map_or(i32::MIN, |o| o as i32), ty));
            }
        }
    }
}
