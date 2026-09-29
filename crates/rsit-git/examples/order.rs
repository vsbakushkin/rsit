//! Prints commit ids in rsit's display order (compare with `git log --date-order`).
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    let local = repo.local();
    let refs = rsit_git::read_refs(&local)?;
    let data = rsit_git::load_commit_graph(&local, &refs.tips(), None)?;
    for id in &data.ids {
        println!("{id}");
    }
    Ok(())
}
