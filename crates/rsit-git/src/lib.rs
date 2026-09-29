//! Git access: gix for reads, the system `git` for writes and a few reads that
//! gix does not cover yet.

pub mod changes;
pub mod cli;
mod graph;
pub mod history;
pub mod ops;
mod refs;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
pub use gix::ObjectId;

pub use graph::{CommitGraphData, MISSING, load_commit_graph};
pub use refs::{Ref, RefKind, Refs, read_refs};

/// A repository opened for reading. Cheap to clone; clones share object caches
/// only through the underlying object database.
#[derive(Clone)]
pub struct Repo {
    repo: gix::ThreadSafeRepository,
    workdir: Option<PathBuf>,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl Repo {
    /// Finds the repository containing `path`.
    pub fn discover(path: &Path) -> Result<Self> {
        // absolute paths keep file watching and `git` invocations independent of the cwd
        let path = path.canonicalize().with_context(|| format!("no such path: {}", path.display()))?;
        let repo = gix::discover(&path).with_context(|| format!("not a git repository: {}", path.display()))?;
        let workdir = repo.workdir().map(Path::to_path_buf);
        let git_dir = repo.git_dir().to_path_buf();
        let common_dir = repo.common_dir().to_path_buf();
        Ok(Self { repo: repo.into_sync(), workdir, git_dir, common_dir })
    }

    pub fn local(&self) -> gix::Repository {
        let mut repo = self.repo.to_thread_local();
        repo.object_cache_size_if_unset(16 * 1024 * 1024);
        repo
    }

    pub fn workdir(&self) -> Option<&Path> {
        self.workdir.as_deref()
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Directory with refs and objects shared by all worktrees.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Directory where `git` commands should run.
    pub fn cwd(&self) -> &Path {
        self.workdir.as_deref().unwrap_or(&self.git_dir)
    }

    pub fn display_name(&self) -> String {
        let dir = self.workdir.as_deref().unwrap_or(&self.git_dir);
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Seconds since the unix epoch.
    pub time: i64,
    /// Offset from UTC in seconds.
    pub offset: i32,
}

#[derive(Clone, Debug)]
pub struct CommitMeta {
    pub id: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    pub message: String,
}

impl CommitMeta {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

fn signature(sig: gix::actor::SignatureRef<'_>) -> Signature {
    let time = sig.time().unwrap_or_default();
    Signature { name: sig.name.to_string(), email: sig.email.to_string(), time: time.seconds, offset: time.offset }
}

/// Reads commit metadata. Uses the thread-local repository `repo` so callers can
/// batch many lookups on one object cache.
pub fn read_commit(repo: &gix::Repository, id: ObjectId) -> Result<CommitMeta> {
    let commit = repo.find_commit(id)?;
    let decoded = commit.decode().map_err(decode_err)?;
    Ok(CommitMeta {
        id,
        parents: decoded.parents().collect(),
        author: signature(decoded.author().map_err(decode_err)?),
        committer: signature(decoded.committer().map_err(decode_err)?),
        message: decoded.message.to_string(),
    })
}

fn decode_err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("cannot decode commit: {e:?}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
}

impl ChangeKind {
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Modified => 'M',
            ChangeKind::Renamed => 'R',
            ChangeKind::Copied => 'C',
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub kind: ChangeKind,
    pub path: String,
    /// Source path for renames and copies.
    pub old_path: Option<String>,
}

/// Files changed by `commit` compared to its first parent (or the empty tree).
pub fn changed_files(repo: &gix::Repository, commit: ObjectId) -> Result<Vec<FileChange>> {
    use gix::object::tree::diff::ChangeDetached as C;
    let commit = repo.find_commit(commit)?;
    let new_tree = commit.tree()?;
    let old_tree = match commit.parent_ids().next() {
        Some(parent) => Some(repo.find_commit(parent)?.tree()?),
        None => None,
    };
    let changes = repo.diff_tree_to_tree(old_tree.as_ref(), &new_tree, None)?;
    let mut out = Vec::with_capacity(changes.len());
    for change in changes {
        let (kind, path, old_path, mode) = match change {
            C::Addition { location, entry_mode, .. } => (ChangeKind::Added, location, None, entry_mode),
            C::Deletion { location, entry_mode, .. } => (ChangeKind::Deleted, location, None, entry_mode),
            C::Modification { location, entry_mode, .. } => (ChangeKind::Modified, location, None, entry_mode),
            C::Rewrite { source_location, location, entry_mode, copy, .. } => {
                let kind = if copy { ChangeKind::Copied } else { ChangeKind::Renamed };
                (kind, location, Some(source_location.to_string()), entry_mode)
            }
        };
        if mode.is_tree() {
            continue;
        }
        out.push(FileChange { kind, path: path.to_string(), old_path });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Contents of `path` in `commit`, `None` if the file does not exist there.
pub fn file_at(repo: &gix::Repository, commit: ObjectId, path: &str) -> Result<Option<Vec<u8>>> {
    let tree = repo.find_commit(commit)?.tree()?;
    let Some(entry) = tree.lookup_entry_by_path(path)? else {
        return Ok(None);
    };
    if !entry.mode().is_blob_or_symlink() {
        return Ok(None);
    }
    Ok(Some(entry.object()?.detach().data))
}

/// First parent of `commit`, the side IntelliJ compares against by default.
pub fn first_parent(repo: &gix::Repository, commit: ObjectId) -> Result<Option<ObjectId>> {
    Ok(repo.find_commit(commit)?.parent_ids().next().map(|id| id.detach()))
}

/// Where one side of a diff comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revision {
    Commit(ObjectId),
    /// The staging area.
    Index,
    /// Files on disk.
    WorkTree,
}

/// Contents of `path` at `rev`; `None` if the file does not exist there.
pub fn file_at_revision(repo: &Repo, rev: Revision, path: &str) -> Result<Option<Vec<u8>>> {
    let local = repo.local();
    match rev {
        Revision::Commit(id) => file_at(&local, id, path),
        Revision::Index => {
            let index = local.index_or_empty()?;
            let Some(entry) = index.entry_by_path(path.into()) else {
                return Ok(None);
            };
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                return Ok(None);
            }
            Ok(Some(local.find_object(entry.id)?.detach().data))
        }
        Revision::WorkTree => {
            let Some(workdir) = repo.workdir() else {
                return Ok(None);
            };
            let full = workdir.join(path);
            match std::fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    Ok(Some(std::fs::read_link(&full)?.to_string_lossy().into_owned().into_bytes()))
                }
                Ok(meta) if meta.is_file() => Ok(Some(std::fs::read(&full)?)),
                Ok(_) => Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
    }
}
