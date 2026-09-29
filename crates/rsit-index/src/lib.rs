//! On-disk caches keyed by repository: the commit graph (ids, times, parents),
//! so a restart only walks commits created since the last run.

use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use gix::ObjectId;
use rsit_git::{CommitGraphData, Repo};

const GRAPH_MAGIC: &[u8; 8] = b"RSITGR01";

/// Cache directory of a repository: `$XDG_CACHE_HOME/rsit/<hash of the common dir>`.
pub fn cache_dir(repo: &Repo) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let key = repo.common_dir().canonicalize().unwrap_or_else(|_| repo.common_dir().to_path_buf());
    Some(base.join("rsit").join(format!("{:016x}", fnv1a(key.as_os_str().as_encoded_bytes()))))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn graph_path(repo: &Repo) -> Option<PathBuf> {
    Some(cache_dir(repo)?.join("graph.bin"))
}

/// Reads the cached graph; `None` when there is no usable cache.
pub fn read_graph(path: &Path) -> Option<CommitGraphData> {
    read_graph_file(path).ok()
}

fn read_graph_file(path: &Path) -> Result<CommitGraphData> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.read_to_end(&mut bytes)?;
    let mut r = Reader { bytes: &bytes, pos: 0 };
    ensure!(r.take(8)? == GRAPH_MAGIC, "bad magic");
    let n = r.u32()? as usize;
    let m = r.u32()? as usize;
    let mut ids = Vec::with_capacity(n);
    for chunk in r.take(20 * n)?.chunks_exact(20) {
        ids.push(ObjectId::from_bytes_or_panic(chunk));
    }
    let times = r.take(8 * n)?.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect();
    let offsets: Vec<u32> = r.u32s(n + 1)?;
    let list: Vec<u32> = r.u32s(m)?;
    ensure!(r.pos == bytes.len(), "trailing bytes");
    ensure!(offsets.last().copied() == Some(m as u32), "bad offsets");
    ensure!(offsets.windows(2).all(|w| w[0] <= w[1]), "bad offsets");
    ensure!(list.iter().all(|&p| p == rsit_git::MISSING || (p as usize) < n), "bad parent");
    Ok(CommitGraphData::from_parts(ids, times, offsets, list))
}

/// Writes the graph atomically (temp file + rename).
pub fn write_graph(path: &Path, data: &CommitGraphData) -> Result<()> {
    let (ids, times, offsets, list) = data.parts();
    if ids.iter().any(|id| id.kind() != gix::hash::Kind::Sha1) {
        bail!("only SHA-1 repositories are cached");
    }
    let dir = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut w = BufWriter::with_capacity(1 << 20, fs::File::create(&tmp)?);
        w.write_all(GRAPH_MAGIC)?;
        w.write_all(&(ids.len() as u32).to_le_bytes())?;
        w.write_all(&(list.len() as u32).to_le_bytes())?;
        for id in ids {
            w.write_all(id.as_bytes())?;
        }
        for t in times {
            w.write_all(&t.to_le_bytes())?;
        }
        for v in offsets.iter().chain(list) {
            w.write_all(&v.to_le_bytes())?;
        }
        w.flush()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).filter(|&e| e <= self.bytes.len()).context("truncated cache")?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u32s(&mut self, count: usize) -> Result<Vec<u32>> {
        Ok(self.take(4 * count)?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let id = |b: u8| ObjectId::from_bytes_or_panic(&[b; 20]);
        let data = CommitGraphData::from_parts(
            vec![id(1), id(2), id(3)],
            vec![30, 20, 10],
            vec![0, 1, 3, 3],
            vec![1, 2, rsit_git::MISSING],
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.bin");
        write_graph(&path, &data).unwrap();
        let back = read_graph(&path).unwrap();
        assert_eq!(back.ids, data.ids);
        assert_eq!(back.times, data.times);
        assert_eq!(back.parents(1), &[2, rsit_git::MISSING]);
        assert_eq!(back.index[&id(3)], 2);

        std::fs::write(&path, b"RSITGR01garbage").unwrap();
        assert!(read_graph(&path).is_none());
    }
}
