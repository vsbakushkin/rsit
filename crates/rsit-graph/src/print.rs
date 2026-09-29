//! Port of IntelliJ `PrintElementGeneratorImpl` and `GraphElementComparatorByLayoutIndex`:
//! turns a [`LinearGraph`] into per-row drawing primitives, hiding the middle
//! part of long edges behind arrows.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::edges_in_row::EdgesInRowGenerator;
use crate::linear::{EdgeFilter, EdgeType, GraphEdge, GraphElement, LinearGraph};

pub const LONG_EDGE_SIZE: u32 = 30;
const LONG_EDGE_PART_SIZE: u32 = 1;
const VERY_LONG_EDGE_SIZE: u32 = 1000;
const VERY_LONG_EDGE_PART_SIZE: u32 = 250;

const CACHE_SIZE: usize = 100;
const SAMPLE_SIZE: usize = 20000;
const K: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeDir {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintKind {
    Node,
    /// Half of an edge between this row and the row above/below.
    Edge { other_pos: u32, dir: EdgeDir, arrow: bool },
    /// Arrow stub of a special edge that has no end in the neighbour row.
    Terminal { dir: EdgeDir },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintElement {
    pub row: u32,
    pub pos: u32,
    pub kind: PrintKind,
    pub element: GraphElement,
    pub color_id: i32,
}

impl PrintElement {
    pub fn other_pos(&self) -> u32 {
        match self.kind {
            PrintKind::Edge { other_pos, .. } => other_pos,
            _ => self.pos,
        }
    }

    pub fn dashed(&self) -> bool {
        match self.element {
            GraphElement::Edge(e) => !matches!(e.ty, EdgeType::Usual | EdgeType::NotLoadCommit),
            GraphElement::Node(_) => false,
        }
    }
}

/// Maps graph elements to color ids (see `PrintElementPresentationManager`).
pub trait ColorIds {
    fn color_id(&self, element: &GraphElement) -> i32;
}

impl<F: Fn(&GraphElement) -> i32> ColorIds for F {
    fn color_id(&self, element: &GraphElement) -> i32 {
        self(element)
    }
}

pub struct PrintElementGenerator {
    long_edge_size: u32,
    visible_part_size: u32,
    edge_with_arrow_size: u32,
    cache: HashMap<u32, Vec<GraphElement>>,
    edges_in_row: EdgesInRowGenerator,
}

impl PrintElementGenerator {
    pub fn new(show_long_edges: bool) -> Self {
        if show_long_edges {
            Self::with_sizes(VERY_LONG_EDGE_SIZE, VERY_LONG_EDGE_PART_SIZE, LONG_EDGE_SIZE)
        } else {
            Self::with_sizes(LONG_EDGE_SIZE, LONG_EDGE_PART_SIZE, u32::MAX)
        }
    }

    pub fn with_sizes(long_edge_size: u32, visible_part_size: u32, edge_with_arrow_size: u32) -> Self {
        Self {
            long_edge_size,
            visible_part_size,
            edge_with_arrow_size,
            cache: HashMap::new(),
            edges_in_row: EdgesInRowGenerator::default(),
        }
    }

    pub fn invalidate(&mut self) {
        self.cache.clear();
        self.edges_in_row.invalidate();
    }

    pub fn print_elements(
        &mut self,
        graph: &impl LinearGraph,
        layout_index: &impl Fn(u32) -> u32,
        colors: &impl ColorIds,
        row: u32,
    ) -> Vec<PrintElement> {
        let mut result = Vec::new();
        let mut nodes = Vec::new();
        let visible = self.sorted_visible_elements(graph, layout_index, row);
        let up_pos = self.end_positions(graph, layout_index, row as i64 - 1);
        let down_pos = self.end_positions(graph, layout_index, row as i64 + 1);
        let find = |map: &Option<HashMap<GraphElement, u32>>, edge: &GraphEdge, up: bool| -> Option<u32> {
            let map = map.as_ref()?;
            map.get(&GraphElement::Edge(*edge)).copied().or_else(|| {
                let node = if up { edge.up } else { edge.down }?;
                map.get(&GraphElement::Node(node)).copied()
            })
        };
        let push_edge = |result: &mut Vec<PrintElement>, pos: u32, edge: GraphEdge, other_pos: Option<u32>, dir, arrow: Option<EdgeDir>, terminal: bool| {
            let element = GraphElement::Edge(edge);
            let color_id = colors.color_id(&element);
            if let Some(other_pos) = other_pos {
                let kind = PrintKind::Edge { other_pos, dir, arrow: arrow == Some(dir) };
                result.push(PrintElement { row, pos, kind, element, color_id });
            } else if terminal && arrow == Some(dir) {
                result.push(PrintElement { row, pos, kind: PrintKind::Terminal { dir }, element, color_id });
            }
        };

        let mut adjacent = Vec::new();
        for (pos, element) in visible.iter().enumerate() {
            let pos = pos as u32;
            match *element {
                GraphElement::Node(node) => {
                    nodes.push(PrintElement {
                        row,
                        pos,
                        kind: PrintKind::Node,
                        element: *element,
                        color_id: colors.color_id(element),
                    });
                    adjacent.clear();
                    graph.adjacent_edges(node, EdgeFilter::ALL, &mut adjacent);
                    for edge in &adjacent {
                        let arrow = self.arrow_type(edge, row);
                        let down = find(&down_pos, edge, false);
                        let up = find(&up_pos, edge, true);
                        if down.is_some() {
                            push_edge(&mut result, pos, *edge, down, EdgeDir::Down, arrow, false);
                        }
                        if up.is_some() {
                            push_edge(&mut result, pos, *edge, up, EdgeDir::Up, arrow, false);
                        }
                    }
                }
                GraphElement::Edge(edge) => {
                    let arrow = self.arrow_type(&edge, row);
                    let down = find(&down_pos, &edge, false);
                    let up = find(&up_pos, &edge, true);
                    push_edge(&mut result, pos, edge, down, EdgeDir::Down, arrow, true);
                    push_edge(&mut result, pos, edge, up, EdgeDir::Up, arrow, true);
                }
            }
        }
        result.extend(nodes);
        result
    }

    fn end_positions(
        &mut self,
        graph: &impl LinearGraph,
        layout_index: &impl Fn(u32) -> u32,
        row: i64,
    ) -> Option<HashMap<GraphElement, u32>> {
        if row < 0 || row >= graph.nodes_count() as i64 {
            return None;
        }
        let elements = self.sorted_visible_elements(graph, layout_index, row as u32);
        Some(elements.into_iter().enumerate().map(|(pos, e)| (e, pos as u32)).collect())
    }

    fn arrow_type(&self, edge: &GraphEdge, row: u32) -> Option<EdgeDir> {
        if let Some((up, down)) = edge.as_normal() {
            return self.normal_arrow_type(up, down, row);
        }
        match edge.ty {
            EdgeType::DottedArrowDown | EdgeType::NotLoadCommit => {
                (row > 0 && edge.up == Some(row - 1)).then_some(EdgeDir::Down)
            }
            EdgeType::DottedArrowUp => (edge.down == Some(row + 1)).then_some(EdgeDir::Up),
            _ => None,
        }
    }

    fn normal_arrow_type(&self, up: u32, down: u32, row: u32) -> Option<EdgeDir> {
        let size = down - up;
        let up_offset = row as i64 - up as i64;
        let down_offset = down as i64 - row as i64;
        if size >= self.long_edge_size {
            if up_offset == self.visible_part_size as i64 {
                return Some(EdgeDir::Down);
            }
            if down_offset == self.visible_part_size as i64 {
                return Some(EdgeDir::Up);
            }
        }
        if size >= self.edge_with_arrow_size {
            if up_offset == 1 {
                return Some(EdgeDir::Down);
            }
            if down_offset == 1 {
                return Some(EdgeDir::Up);
            }
        }
        None
    }

    fn is_edge_visible_in_row(&self, up: u32, down: u32, row: u32) -> bool {
        down - up < self.long_edge_size || (row - up).min(down - row) <= self.visible_part_size
    }

    fn sorted_visible_elements(
        &mut self,
        graph: &impl LinearGraph,
        layout_index: &impl Fn(u32) -> u32,
        row: u32,
    ) -> Vec<GraphElement> {
        if let Some(e) = self.cache.get(&row) {
            return e.clone();
        }
        let mut result = vec![GraphElement::Node(row)];
        let mut edges: Vec<GraphEdge> = self.edges_in_row.edges_in_row(graph, row as usize).into_iter().collect();
        // HashSet iteration order is random; sort for determinism before the stable sort below
        edges.sort_by_key(|e| (e.up, e.down, e.target_id, e.ty));
        result.extend(
            edges
                .into_iter()
                .filter(|e| e.as_normal().is_some_and(|(u, d)| self.is_edge_visible_in_row(u, d, row)))
                .map(GraphElement::Edge),
        );
        let mut special = Vec::new();
        if row > 0 {
            graph.adjacent_edges(row - 1, EdgeFilter::SPECIAL, &mut special);
            result.extend(special.drain(..).filter(|e| e.is_down_for(row - 1)).map(GraphElement::Edge));
        }
        if (row as usize) + 1 < graph.nodes_count() {
            graph.adjacent_edges(row + 1, EdgeFilter::SPECIAL, &mut special);
            result.extend(special.drain(..).filter(|e| e.is_up_for(row + 1)).map(GraphElement::Edge));
        }
        insertion_sort(&mut result, |a, b| compare_elements(a, b, layout_index));
        if self.cache.len() >= CACHE_SIZE * 2 {
            self.cache.clear();
        }
        self.cache.insert(row, result.clone());
        result
    }

    /// Port of `calculateRecommendedWidth`: a width (in lanes) that fits most rows.
    pub fn recommended_width(&self, graph: &impl LinearGraph) -> u32 {
        let count = graph.nodes_count();
        if count <= 1 {
            return count as u32;
        }
        let n = SAMPLE_SIZE.min(count);
        let (mut sum, mut sum_sq) = (0.0f64, 0.0f64);
        let mut edges_count = 0usize;
        let mut current: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut adjacent = Vec::new();
        for i in 0..n {
            let row = i as u32;
            adjacent.clear();
            graph.adjacent_edges(row, EdgeFilter::ALL, &mut adjacent);
            let (mut up_arrows, mut down_arrows) = (0usize, 0usize);
            for e in &adjacent {
                match e.as_normal() {
                    Some(normal) => {
                        if e.is_up_for(row) {
                            current.remove(&normal);
                        } else {
                            current.insert(normal);
                        }
                    }
                    None if e.ty == EdgeType::DottedArrowUp => up_arrows += 1,
                    None => down_arrows += 1,
                }
            }
            let mut new_edges = 0usize;
            for &(up, down) in &current {
                if self.is_edge_visible_in_row(up, down, row) {
                    new_edges += 1;
                } else {
                    match self.normal_arrow_type(up, down, row) {
                        Some(EdgeDir::Down) => down_arrows += 1,
                        Some(EdgeDir::Up) => up_arrows += 1,
                        None => {}
                    }
                }
            }
            let width = (edges_count + up_arrows).max(new_edges + down_arrows) as f64;
            let weight = 2.0 / (n as f64 * (K + 1.0)) * (1.0 + (K - 1.0) * i as f64 / (n as f64 - 1.0));
            sum += width * weight;
            sum_sq += width * width * weight;
            edges_count = new_edges;
        }
        let deviation = (sum_sq - sum * sum).max(0.0).sqrt();
        (sum + deviation).round() as u32
    }
}

/// Stable insertion sort: never panics on comparators that are not a strict total order.
fn insertion_sort<T: Copy>(v: &mut [T], mut cmp: impl FnMut(&T, &T) -> Ordering) {
    for i in 1..v.len() {
        let x = v[i];
        let mut j = i;
        while j > 0 && cmp(&x, &v[j - 1]) == Ordering::Less {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

pub fn compare_elements(o1: &GraphElement, o2: &GraphElement, li: &impl Fn(u32) -> u32) -> Ordering {
    let r = match (o1, o2) {
        (GraphElement::Edge(e1), GraphElement::Edge(e2)) => match (e1.as_normal(), e2.as_normal()) {
            (None, _) => -compare_edge_node(e2, e1.not_null_node(), li),
            (_, None) => compare_edge_node(e1, e2.not_null_node(), li),
            (Some((up1, down1)), Some((up2, down2))) => {
                if up1 == up2 {
                    if down1 < down2 {
                        -compare_edge_node(e2, down1, li)
                    } else {
                        compare_edge_node(e1, down2, li)
                    }
                } else if up1 < up2 {
                    compare_edge_node(e1, up2, li)
                } else {
                    -compare_edge_node(e2, up1, li)
                }
            }
        },
        (GraphElement::Edge(e), GraphElement::Node(n)) => compare_edge_node(e, *n, li),
        (GraphElement::Node(n), GraphElement::Edge(e)) => -compare_edge_node(e, *n, li),
        (GraphElement::Node(_), GraphElement::Node(_)) => 0,
    };
    r.cmp(&0)
}

fn compare_edge_node(edge: &GraphEdge, node: u32, li: &impl Fn(u32) -> u32) -> i64 {
    let Some((up, down)) = edge.as_normal() else {
        return li(edge.not_null_node()) as i64 - li(node) as i64;
    };
    let edge_li = li(up).max(li(down)) as i64;
    let node_li = li(node) as i64;
    if edge_li != node_li { edge_li - node_li } else { up as i64 - node as i64 }
}
