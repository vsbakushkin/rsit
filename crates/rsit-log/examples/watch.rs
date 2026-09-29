//! Prints ref change notifications for a repository.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let repo = rsit_git::Repo::discover(path.as_ref())?;
    println!("git_dir={} common={}", repo.git_dir().display(), repo.common_dir().display());
    let (_w, mut rx) = rsit_log::watch::watch_refs(&repo)?;
    futures::executor::block_on(async {
        use futures::StreamExt;
        while rx.next().await.is_some() {
            let data = rsit_log::LogData::load(repo.clone(), None).unwrap();
            println!("changed: {} commits, {} refs", data.len(), data.refs.refs.len());
        }
    });
    Ok(())
}
