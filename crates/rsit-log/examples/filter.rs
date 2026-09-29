//! `filter <repo> <text> [user]`: times a text/user filter over the whole history.
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".into());
    let text = args.next().unwrap_or_default();
    let user = args.next().unwrap_or_default();
    let data = rsit_log::LogData::load(rsit_git::Repo::discover(path.as_ref())?, None)?;
    let filter = rsit_log::LogFilter { text, user, ..Default::default() };
    let t = std::time::Instant::now();
    let rows = rsit_log::filter::visible_rows(&data, &filter)?.unwrap();
    println!("{} matches in {:?}", rows.iter().filter(|&&v| v).count(), t.elapsed());
    Ok(())
}
