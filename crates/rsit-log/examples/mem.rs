//! Prints peak RSS after loading a repository (model only, no UI).
fn rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("VmRSS") || l.starts_with("VmHWM"))
        .collect::<Vec<_>>()
        .join(" ")
}
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    println!("start: {}", rss());
    let data = rsit_log::LogData::load(repo, None)?;
    println!("loaded {}: {}", data.len(), rss());
    let graph = rsit_log::VisibleGraph::new(std::sync::Arc::new(data), None);
    println!("visible graph ({} lanes): {}", graph.recommended_width(), rss());
    Ok(())
}
