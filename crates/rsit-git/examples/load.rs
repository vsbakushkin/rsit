//! `cargo run --release -p rsit-git --example load -- <repo>`: times graph loading.
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let t = Instant::now();
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    let local = repo.local();
    let refs = rsit_git::read_refs(&local)?;
    println!("refs: {} in {:?}", refs.refs.len(), t.elapsed());
    let t = Instant::now();
    let first = rsit_git::load_commit_graph(&local, &refs.tips(), Some(1000), None)?;
    println!("first {} commits in {:?}", first.len(), t.elapsed());
    let t = Instant::now();
    let data = rsit_git::load_commit_graph(&local, &refs.tips(), None, None)?;
    println!("all {} commits in {:?}", data.len(), t.elapsed());
    let t = Instant::now();
    let commits: Vec<rsit_graph::GraphCommit<u32>> = (0..data.len())
        .map(|i| rsit_graph::GraphCommit { id: i as u32, parents: data.parents(i as u32).to_vec() })
        .collect();
    let heads: Vec<u32> = refs.tips().iter().filter_map(|t| data.index.get(t).copied()).collect();
    let graph = rsit_graph::PermanentGraph::new(&commits, heads, |a, b| a.cmp(b));
    println!("graph built in {:?} ({} nodes)", t.elapsed(), graph.len());
    let t = Instant::now();
    let mut generator = rsit_graph::PrintElementGenerator::new(false);
    let li = |n: u32| graph.layout.layout_index(n);
    let colors = |e: &rsit_graph::GraphElement| graph.color_id(e, |_| None);
    let mut total = 0;
    for row in (0..graph.len() as u32).step_by((graph.len() / 2000).max(1)) {
        total += generator.print_elements(&graph.linear, &li, &colors, row).len();
    }
    println!("print elements for 2000 sampled rows: {total} in {:?}", t.elapsed());
    Ok(())
}
