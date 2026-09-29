//! Times comparing a large generated file with scattered edits.
fn main() {
    let n = 100_000;
    let left: String = (0..n).map(|i| format!("    let value_{i} = compute({i}, \"text {i}\");\n")).collect();
    let right: String = (0..n)
        .map(|i| {
            if i % 97 == 0 {
                format!("    let value_{i} = compute_fast({i}, \"text {i}\");\n")
            } else {
                format!("    let value_{i} = compute({i}, \"text {i}\");\n")
            }
        })
        .collect();
    let t = std::time::Instant::now();
    let fragments = rsit_diff::compare(&left, &right, rsit_diff::WhitespacePolicy::Default);
    println!("{} fragments in {:?}", fragments.len(), t.elapsed());
    let t = std::time::Instant::now();
    let rows = rsit_diff::build_rows(
        &fragments,
        n,
        n,
        rsit_diff::Layout::SideBySide,
        Some(&rsit_diff::Folding { context: 4, expanded: Default::default() }),
    );
    println!("{} rows in {:?}", rows.rows.len(), t.elapsed());
}
