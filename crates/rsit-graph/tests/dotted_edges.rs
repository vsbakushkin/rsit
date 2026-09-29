//! Port of IntelliJ `DottedFilterEdgesGeneratorTest`.

use rsit_graph::FilteredGraph;
use rsit_graph::linear::{EdgeFilter, EdgeType, GraphCommit, LinearGraph, PermanentLinearGraph};

/// `spec`: (label, matched, children labels) in row order.
fn check(spec: &[(u32, bool, &[u32])], expected: &str) {
    let commits: Vec<GraphCommit<u32>> =
        spec.iter().map(|(id, _, parents)| GraphCommit { id: *id, parents: parents.to_vec() }).collect();
    let graph = PermanentLinearGraph::build(&commits, |_| i32::MIN);
    let visible: Vec<bool> = spec.iter().map(|s| s.1).collect();
    let filtered = FilteredGraph::new(&graph, &visible);
    let view = filtered.view(&graph);
    let label = |row: u32| spec[filtered.to_delegate(row) as usize].0;
    let mut lines = Vec::new();
    for row in 0..view.nodes_count() as u32 {
        let mut downs: Vec<(u32, EdgeType)> =
            view.edges(row, EdgeFilter::NORMAL_DOWN).iter().map(|e| (label(e.down.unwrap()), e.ty)).collect();
        downs.sort_by_key(|d| d.0);
        let downs: Vec<String> = downs
            .iter()
            .map(|(l, t)| if *t == EdgeType::Dotted { format!("{l}.dot") } else { l.to_string() })
            .collect();
        lines.push(format!("{}({})", label(row), downs.join(", ")));
    }
    assert_eq!(lines.join("\n"), expected.trim());
}

const U: bool = true;
const UNM: bool = false;

#[test]
fn simple() {
    check(&[(1, U, &[2]), (2, UNM, &[3]), (3, U, &[])], "1(3.dot)\n3()");
}

#[test]
fn simple_2_up() {
    check(&[(1, U, &[3]), (2, U, &[3]), (3, UNM, &[4]), (4, U, &[])], "1(4.dot)\n2(4.dot)\n4()");
}

#[test]
fn simple_2_down() {
    check(&[(1, U, &[2]), (2, UNM, &[3, 4]), (3, U, &[]), (4, U, &[])], "1(3.dot, 4.dot)\n3()\n4()");
}

#[test]
fn down_tree() {
    check(
        &[(0, U, &[1, 2]), (1, UNM, &[3, 4]), (2, UNM, &[4, 5]), (3, U, &[]), (4, U, &[]), (5, U, &[])],
        "0(3.dot, 4.dot, 5.dot)\n3()\n4()\n5()",
    );
}

#[test]
fn up_tree() {
    check(
        &[(0, U, &[3]), (1, U, &[3, 4]), (2, U, &[4]), (3, UNM, &[5]), (4, UNM, &[5]), (5, U, &[])],
        "0(5.dot)\n1(5.dot)\n2(5.dot)\n5()",
    );
}

#[test]
fn simple_merge() {
    check(
        &[(1, U, &[2, 3]), (2, U, &[4]), (3, U, &[5]), (4, UNM, &[6]), (5, UNM, &[6]), (6, U, &[])],
        "1(2, 3)\n2(6.dot)\n3(6.dot)\n6()",
    );
}

#[test]
fn simple_merge_2() {
    check(
        &[(1, U, &[2, 3]), (2, UNM, &[4]), (3, UNM, &[5]), (4, U, &[6]), (5, U, &[6]), (6, U, &[])],
        "1(4.dot, 5.dot)\n4(6)\n5(6)\n6()",
    );
}

#[test]
fn fork() {
    check(
        &[(0, U, &[2]), (1, U, &[3]), (2, U, &[3]), (3, UNM, &[4]), (4, UNM, &[5]), (5, U, &[])],
        "0(2)\n1(5.dot)\n2(5.dot)\n5()",
    );
}

#[test]
fn containing_branches() {
    // 0 -> 2, 1 -> 2, 2 -> 3 ; heads 0 and 1
    let commits: Vec<GraphCommit<u32>> = [(0, vec![2]), (1, vec![2]), (2, vec![3]), (3, vec![])]
        .into_iter()
        .map(|(id, parents)| GraphCommit { id, parents })
        .collect();
    let graph = rsit_graph::PermanentGraph::new(&commits, [0, 1], |a, b| a.cmp(b));
    let heads = [0u32, 1].into_iter().collect();
    assert_eq!(graph.containing_branches(3, &heads), vec![0, 1]);
    assert_eq!(graph.containing_branches(1, &heads), vec![1]);
}
