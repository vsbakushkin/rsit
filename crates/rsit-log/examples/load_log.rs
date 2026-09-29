//! Times `LogData::load` (with the graph cache) on a repository.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    println!("cache: {:?}", rsit_index::graph_path(&repo));
    for _ in 0..2 {
        let t = std::time::Instant::now();
        let data = rsit_log::LogData::load(repo.clone(), None)?;
        println!("{} commits in {:?}", data.len(), t.elapsed());
    }
    Ok(())
}
