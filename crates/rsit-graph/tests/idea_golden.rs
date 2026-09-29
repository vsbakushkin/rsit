//! Golden tests on IntelliJ's `platform/vcs-log/graph/testData` (Apache-2.0, JetBrains).

use std::collections::HashMap;

use rsit_graph::linear::{EdgeFilter, EdgeType, GraphCommit, GraphEdge, GraphElement, LinearGraph, PermanentLinearGraph};
use rsit_graph::print::{EdgeDir, PrintElementGenerator, PrintKind};
use rsit_graph::{GraphLayout, edges_in_row::EdgesInRowGenerator};

const SEP: &str = "|-";

fn load(dir: &str, name: &str) -> (String, String) {
    let base = format!("{}/tests/data/{dir}/{name}", env!("CARGO_MANIFEST_DIR"));
    let read = |suffix: &str| {
        std::fs::read_to_string(format!("{base}_{suffix}.txt")).unwrap().replace("\r\n", "\n")
    };
    (read("in"), read("out"))
}

fn lines(s: &str) -> impl Iterator<Item = &str> {
    s.split('\n').filter(|l| !l.is_empty())
}

// ---- commit list format: "a0|-a1 a2" ----

fn parse_commits(input: &str) -> Vec<GraphCommit<String>> {
    lines(input)
        .map(|line| {
            let (id, parents) = line.split_once(SEP).unwrap();
            GraphCommit {
                id: id.to_string(),
                parents: parents.split_whitespace().map(str::to_string).collect(),
            }
        })
        .collect()
}

// ---- linear graph format: "0_U|-1_U 2_D" ----

struct TestGraph {
    node_types: Vec<char>,
    up_edges: HashMap<u32, Vec<GraphEdge>>,
    down_edges: HashMap<u32, Vec<GraphEdge>>,
}

fn parse_num_char(s: &str) -> (i32, char) {
    let (num, ch) = s.split_at(s.len() - 2);
    (num.parse().unwrap(), ch.chars().nth(1).unwrap())
}

fn edge_type(c: char) -> EdgeType {
    match c {
        'U' => EdgeType::Usual,
        'D' => EdgeType::Dotted,
        'N' => EdgeType::NotLoadCommit,
        'P' => EdgeType::DottedArrowUp,
        'O' => EdgeType::DottedArrowDown,
        _ => panic!("edge type {c}"),
    }
}

fn edge_char(t: EdgeType) -> char {
    match t {
        EdgeType::Usual => 'U',
        EdgeType::Dotted => 'D',
        EdgeType::NotLoadCommit => 'N',
        EdgeType::DottedArrowUp => 'P',
        EdgeType::DottedArrowDown => 'O',
    }
}

impl TestGraph {
    fn parse(input: &str) -> Self {
        let mut node_types = Vec::new();
        let mut id_to_index = HashMap::new();
        let mut raw = Vec::new();
        for line in lines(input) {
            let (node, edges) = line.split_once(SEP).unwrap();
            let (id, ty) = parse_num_char(node);
            id_to_index.insert(id, node_types.len() as u32);
            node_types.push(ty);
            raw.push(edges.split_whitespace().map(str::to_string).collect::<Vec<_>>());
        }
        let mut up_edges: HashMap<u32, Vec<GraphEdge>> = HashMap::new();
        let mut down_edges: HashMap<u32, Vec<GraphEdge>> = HashMap::new();
        for (i, edges) in raw.iter().enumerate() {
            for e in edges {
                let (id, c) = parse_num_char(e);
                let ty = edge_type(c);
                let edge = if ty.is_normal() {
                    GraphEdge::normal(i as u32, id_to_index[&id], ty)
                } else {
                    GraphEdge::with_target(i as u32, id, ty)
                };
                if let Some(up) = edge.up {
                    down_edges.entry(up).or_default().push(edge);
                }
                if let Some(down) = edge.down {
                    up_edges.entry(down).or_default().push(edge);
                }
            }
        }
        Self { node_types, up_edges, down_edges }
    }
}

impl LinearGraph for TestGraph {
    fn nodes_count(&self) -> usize {
        self.node_types.len()
    }

    fn adjacent_edges(&self, node: u32, filter: EdgeFilter, out: &mut Vec<GraphEdge>) {
        let all = self.up_edges.get(&node).into_iter().flatten().chain(self.down_edges.get(&node).into_iter().flatten());
        for e in all {
            let pass = match e.as_normal() {
                None => filter.special,
                Some((_, down)) if down == node => filter.up_normal,
                Some(_) => filter.down_normal,
            };
            if pass {
                out.push(*e);
            }
        }
    }
}

fn opt(v: Option<impl ToString>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "n".into())
}

fn edge_str(e: &GraphEdge) -> String {
    format!("{}:{}:{}_{}", opt(e.up), opt(e.down), opt(e.target_id), edge_char(e.ty))
}

// ---- graph builder ----

fn graph_builder_test(name: &str) {
    let (input, expected) = load("graphBuilder", name);
    let commits = parse_commits(&input);
    let graph = PermanentLinearGraph::build(&commits, |id| -i32::from_str_radix(id, 16).unwrap());
    let actual: Vec<String> = (0..graph.nodes_count() as u32)
        .map(|i| {
            let edges: Vec<String> = graph.edges(i, EdgeFilter::ALL).iter().map(edge_str).collect();
            format!("{i}_U{SEP}{}", edges.join(" "))
        })
        .collect();
    assert_eq!(actual.join("\n"), expected.trim_end_matches('\n'), "graphBuilder/{name}");
}

#[test]
fn graph_builder() {
    for name in [
        "simple",
        "manyNodes",
        "manyUpNodes",
        "manyDownNodes",
        "oneNode",
        "oneNodeNotFullGraph",
        "notFullGraph",
        "parentsOrder",
        "duplicateParents",
    ] {
        graph_builder_test(name);
    }
}

// ---- layout builder ----

#[test]
fn layout_builder() {
    for name in ["manyNodes", "notFullGraph", "oneNode", "oneNodeNotFullGraph", "headsOrder"] {
        let (input, expected) = load("layoutBuilder", name);
        let commits = parse_commits(&input);
        let graph = PermanentLinearGraph::build(&commits, |_| i32::MIN);
        let layout = GraphLayout::build(&graph, [], |a, b| commits[a as usize].id.cmp(&commits[b as usize].id));
        let actual: Vec<String> = (0..graph.nodes_count() as u32)
            .map(|i| format!("{}{SEP}{}", layout.layout_index(i), layout.one_of_head_nodes(i)))
            .collect();
        assert_eq!(actual.join("\n"), expected.trim_end_matches('\n'), "layoutBuilder/{name}");
    }
}

#[test]
fn layout_heads_order() {
    // GraphLayoutBuilderHeadOrderTest.branchingGraph
    let commits: Vec<GraphCommit<u32>> = [(0, vec![2]), (1, vec![3]), (2, vec![3]), (3, vec![4]), (4, vec![])]
        .into_iter()
        .map(|(id, parents)| GraphCommit { id, parents })
        .collect();
    let graph = PermanentLinearGraph::build(&commits, |_| i32::MIN);
    let check = |order: &[u32], expected: [u32; 5]| {
        let pos = |n: u32| order.iter().position(|&x| x == n).unwrap();
        let layout = GraphLayout::build(&graph, [0, 2], |a, b| pos(a).cmp(&pos(b)));
        let heads: Vec<u32> = (0..5).map(|i| layout.one_of_head_nodes(i)).collect();
        assert_eq!(heads, expected, "order {order:?}");
    };
    check(&[0, 1, 2], [0, 1, 0, 0, 0]);
    check(&[0, 2, 1], [0, 1, 0, 0, 0]);
    check(&[1, 0, 2], [0, 1, 0, 1, 1]);
    check(&[1, 2, 0], [0, 1, 2, 1, 1]);
    check(&[2, 0, 1], [0, 1, 2, 2, 2]);
    check(&[2, 1, 0], [0, 1, 2, 2, 2]);
}

// ---- edges in row ----

#[test]
fn edges_in_row() {
    for name in [
        "simple",
        "manyNodes",
        "manyUpNodes",
        "manyDownNodes",
        "oneNode",
        "oneNodeNotFullGraph",
        "notFullGraph",
        "notLoadNode",
        "longGraph",
    ] {
        let (input, expected) = load("edgesInRow", name);
        let graph = TestGraph::parse(&input);
        let mut generator = EdgesInRowGenerator::default();
        let actual: Vec<String> = (0..graph.nodes_count())
            .map(|row| {
                let mut edges: Vec<GraphEdge> = generator.edges_in_row(&graph, row).into_iter().collect();
                if edges.is_empty() {
                    return "none".to_string();
                }
                // GraphStrUtils.GRAPH_ELEMENT_COMPARATOR
                edges.sort_by(|a, b| {
                    rsit_graph::print::compare_elements(&GraphElement::Edge(*a), &GraphElement::Edge(*b), &|_| 0)
                });
                edges.iter().map(|e| format!("{}_{}_{}", opt(e.up), opt(e.down), edge_char(e.ty))).collect::<Vec<_>>().join(" ")
            })
            .collect();
        assert_eq!(actual.join("\n"), expected.trim_end_matches('\n'), "edgesInRow/{name}");
    }
}

// ---- print element generator ----

fn element_generator_test(name: &str, long_edge: u32, visible_part: u32, arrow_size: u32) {
    let (input, expected) = load("elementGenerator", name);
    let graph = TestGraph::parse(&input);
    let layout = GraphLayout::build(&graph, [], |a, b| a.cmp(&b));
    let li = |n: u32| layout.layout_index(n);
    let colors = |e: &GraphElement| match e {
        GraphElement::Node(n) => *n as i32,
        GraphElement::Edge(e) => match e.as_normal() {
            Some((u, d)) => (u + d) as i32,
            None => e.not_null_node() as i32,
        },
    };
    let mut generator = PrintElementGenerator::with_sizes(long_edge, visible_part, arrow_size);
    let element_str = |e: &GraphElement| match e {
        GraphElement::Node(n) => format!("{n}_{}", graph.node_types[*n as usize]),
        GraphElement::Edge(e) => edge_str(e),
    };
    let mut rows = Vec::new();
    for row in 0..graph.nodes_count() as u32 {
        let mut elements = generator.print_elements(&graph, &li, &colors, row);
        elements.sort_by_key(|p| match p.kind {
            PrintKind::Node => 1024 * p.pos,
            PrintKind::Edge { dir, .. } | PrintKind::Terminal { dir } => {
                1024 * p.pos + (dir as u32 + 1) * 64 + p.other_pos()
            }
        });
        let strs: Vec<String> = elements
            .iter()
            .map(|p| {
                let el = element_str(&p.element);
                match p.kind {
                    PrintKind::Node => format!("Node{SEP}{row}:{}{SEP}{}:Unselect({el})", p.pos, p.color_id),
                    PrintKind::Edge { dir, .. } | PrintKind::Terminal { dir } => {
                        let arrow = match p.kind {
                            PrintKind::Edge { arrow, .. } => arrow,
                            _ => true,
                        };
                        let dir = match dir {
                            EdgeDir::Up => "UP",
                            EdgeDir::Down => "DOWN",
                        };
                        let arrow = if arrow { "_ARROW" } else { "" };
                        let style = if p.dashed() { "DASHED" } else { "SOLID" };
                        format!(
                            "Edge:{dir}{arrow}:{style}{SEP}{row}:{}:{}{SEP}{}:Unselect({el})",
                            p.pos,
                            p.other_pos(),
                            p.color_id
                        )
                    }
                }
            })
            .collect();
        rows.push(strs.join("\n  "));
    }
    assert_eq!(rows.join("\n"), expected.trim_end_matches('\n'), "elementGenerator/{name}");
}

#[test]
fn element_generator() {
    for name in ["oneNode", "manyNodes", "longEdges", "specialElements"] {
        element_generator_test(name, 7, 2, 10);
    }
    element_generator_test("oneUpOneDown1", 7, 1, 10);
    element_generator_test("oneUpOneDown2", 10, 1, 10);
}
