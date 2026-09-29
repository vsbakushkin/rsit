//! Prints ids in display order after `LogData::load` (uses the graph cache).
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    let data = rsit_log::LogData::load(repo, None)?;
    for id in &data.commits.ids {
        println!("{id}");
    }
    Ok(())
}
