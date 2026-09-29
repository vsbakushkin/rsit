//! Port of IntelliJ `EdgesInRowGenerator`: normal edges passing through a row
//! (not attached to the row's node). Computed incrementally from cached block
//! boundaries, walking at most `walk_size` rows.

use std::collections::{HashMap, HashSet};

use crate::linear::{EdgeFilter, GraphEdge, LinearGraph};

const CACHE_SIZE: usize = 10;
const BLOCK_SIZE: usize = 40;

#[derive(Clone)]
struct GraphEdges {
    edges: HashSet<GraphEdge>,
    row: i64,
}

pub struct EdgesInRowGenerator {
    walk_size: usize,
    cache_up: HashMap<usize, GraphEdges>,
    cache_down: HashMap<usize, GraphEdges>,
    buf: Vec<GraphEdge>,
}

impl Default for EdgesInRowGenerator {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl EdgesInRowGenerator {
    pub fn new(walk_size: usize) -> Self {
        Self { walk_size, cache_up: HashMap::new(), cache_down: HashMap::new(), buf: Vec::new() }
    }

    pub fn invalidate(&mut self) {
        self.cache_up.clear();
        self.cache_down.clear();
    }

    pub fn edges_in_row(&mut self, graph: &impl LinearGraph, row: usize) -> HashSet<GraphEdge> {
        let mut up = self.neighbor_up(graph, row);
        while up.row < row as i64 {
            self.one_down_step(graph, &mut up);
        }
        let mut down = self.neighbor_down(graph, row);
        while down.row > row as i64 {
            self.one_up_step(graph, &mut down);
        }
        up.edges.extend(down.edges);
        up.edges
    }

    fn neighbor_up(&mut self, graph: &impl LinearGraph, row: usize) -> GraphEdges {
        let index = up_neighbor_index(row);
        if let Some(e) = self.cache_up.get(&index) {
            return e.clone();
        }
        let start = index.saturating_sub(self.walk_size);
        let mut edges = GraphEdges { edges: HashSet::new(), row: start as i64 };
        for _ in start..index {
            self.one_down_step(graph, &mut edges);
        }
        put(&mut self.cache_up, index, edges.clone());
        edges
    }

    fn neighbor_down(&mut self, graph: &impl LinearGraph, row: usize) -> GraphEdges {
        let index = up_neighbor_index(row) + BLOCK_SIZE;
        let count = graph.nodes_count();
        if index >= count {
            return GraphEdges { edges: HashSet::new(), row: count as i64 - 1 };
        }
        if let Some(e) = self.cache_down.get(&index) {
            return e.clone();
        }
        let end = (index + self.walk_size).min(count - 1);
        let mut edges = GraphEdges { edges: HashSet::new(), row: end as i64 };
        for _ in index..end {
            self.one_up_step(graph, &mut edges);
        }
        put(&mut self.cache_down, index, edges.clone());
        edges
    }

    fn one_down_step(&mut self, graph: &impl LinearGraph, e: &mut GraphEdges) {
        let row = e.row as u32;
        self.buf.clear();
        graph.adjacent_edges(row, EdgeFilter::NORMAL_DOWN, &mut self.buf);
        e.edges.extend(self.buf.iter().copied());
        if (row as usize + 1) < graph.nodes_count() {
            self.buf.clear();
            graph.adjacent_edges(row + 1, EdgeFilter::NORMAL_UP, &mut self.buf);
            for edge in &self.buf {
                e.edges.remove(edge);
            }
        }
        e.row += 1;
    }

    fn one_up_step(&mut self, graph: &impl LinearGraph, e: &mut GraphEdges) {
        let row = e.row as u32;
        self.buf.clear();
        graph.adjacent_edges(row, EdgeFilter::NORMAL_UP, &mut self.buf);
        e.edges.extend(self.buf.iter().copied());
        if row > 0 {
            self.buf.clear();
            graph.adjacent_edges(row - 1, EdgeFilter::NORMAL_DOWN, &mut self.buf);
            for edge in &self.buf {
                e.edges.remove(edge);
            }
        }
        e.row -= 1;
    }
}

fn up_neighbor_index(row: usize) -> usize {
    row / BLOCK_SIZE * BLOCK_SIZE
}

fn put(cache: &mut HashMap<usize, GraphEdges>, key: usize, value: GraphEdges) {
    if cache.len() >= CACHE_SIZE * 2 {
        cache.clear();
    }
    cache.insert(key, value);
}
