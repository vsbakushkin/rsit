use std::cmp::Ordering;

use anyhow::Result;
use gix::ObjectId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.0`, `HEAD`.
    pub name: String,
    pub full_name: String,
    /// Commit the ref points to (tags are peeled).
    pub target: ObjectId,
}

#[derive(Clone, Debug, Default)]
pub struct Refs {
    pub refs: Vec<Ref>,
    /// Name of the checked out branch; `None` when HEAD is detached.
    pub current_branch: Option<String>,
    pub head: Option<ObjectId>,
}

impl Refs {
    pub fn at(&self, commit: ObjectId) -> impl Iterator<Item = &Ref> {
        self.refs.iter().filter(move |r| r.target == commit)
    }

    /// All commits that refs point to.
    pub fn tips(&self) -> Vec<ObjectId> {
        let mut tips: Vec<ObjectId> = self.refs.iter().map(|r| r.target).collect();
        tips.sort();
        tips.dedup();
        tips
    }

    /// Port of IntelliJ `GitBranchLayoutComparator`: which branch owns shared
    /// history in the graph layout (smaller = more important).
    pub fn layout_cmp(&self, a: &Ref, b: &Ref) -> Ordering {
        const ORDER: [RefClass; 8] = [
            RefClass::OriginMaster,
            RefClass::RemoteBranch,
            RefClass::Master,
            RefClass::LocalBranch,
            RefClass::Tag,
            RefClass::CurrentBranch,
            RefClass::Head,
            RefClass::Other,
        ];
        self.cmp_by(a, b, &ORDER, false)
    }

    /// Port of IntelliJ `GitLabelComparator`: order of ref labels in the table.
    pub fn label_cmp(&self, a: &Ref, b: &Ref) -> Ordering {
        const ORDER: [RefClass; 8] = [
            RefClass::Head,
            RefClass::CurrentBranch,
            RefClass::Master,
            RefClass::OriginMaster,
            RefClass::LocalBranch,
            RefClass::RemoteBranch,
            RefClass::Tag,
            RefClass::Other,
        ];
        self.cmp_by(a, b, &ORDER, true)
    }

    fn cmp_by(&self, a: &Ref, b: &Ref, order: &[RefClass], detect_current: bool) -> Ordering {
        let class = |r: &Ref| {
            let c = ref_class(r);
            if detect_current
                && matches!(c, RefClass::LocalBranch | RefClass::Master)
                && self.current_branch.as_deref() == Some(r.name.as_str())
            {
                RefClass::CurrentBranch
            } else {
                c
            }
        };
        let pos = |c: RefClass| order.iter().position(|x| *x == c).unwrap_or(order.len());
        pos(class(a)).cmp(&pos(class(b))).then_with(|| refs_names_cmp(&a.name, &b.name))
    }

    /// The ref that names the branch starting at `commit` (`RefsModel.getRefForHeadCommit`).
    pub fn best_ref_at(&self, commit: ObjectId) -> Option<&Ref> {
        self.at(commit).min_by(|a, b| self.layout_cmp(a, b))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefClass {
    Head,
    CurrentBranch,
    Master,
    OriginMaster,
    LocalBranch,
    RemoteBranch,
    Tag,
    Other,
}

fn ref_class(r: &Ref) -> RefClass {
    match r.kind {
        RefKind::Head => RefClass::Head,
        RefKind::Tag => RefClass::Tag,
        RefKind::LocalBranch if r.name == "master" || r.name == "main" => RefClass::Master,
        RefKind::LocalBranch => RefClass::LocalBranch,
        RefKind::RemoteBranch if r.name == "origin/master" || r.name == "origin/main" => RefClass::OriginMaster,
        RefKind::RemoteBranch => RefClass::RemoteBranch,
        RefKind::Other => RefClass::Other,
    }
}

/// `GitReference.REFS_NAMES_COMPARATOR`: case-insensitive, then case-sensitive.
fn refs_names_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

pub fn read_refs(repo: &gix::Repository) -> Result<Refs> {
    let mut out = Refs::default();
    let platform = repo.references()?;
    for reference in platform.all()? {
        let Ok(mut reference) = reference else {
            continue;
        };
        let full_name = reference.name().as_bstr().to_string();
        let (kind, name) = if let Some(n) = full_name.strip_prefix("refs/heads/") {
            (RefKind::LocalBranch, n.to_string())
        } else if let Some(n) = full_name.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            (RefKind::RemoteBranch, n.to_string())
        } else if let Some(n) = full_name.strip_prefix("refs/tags/") {
            (RefKind::Tag, n.to_string())
        } else {
            // stash, notes, bisect, ... are not shown in the log
            continue;
        };
        let Ok(id) = reference.peel_to_id() else {
            continue;
        };
        let target = id.detach();
        // lightweight tags on trees/blobs have no commit
        if kind == RefKind::Tag && !is_commit(repo, target) {
            continue;
        }
        out.refs.push(Ref { kind, name, full_name, target });
    }

    let head = repo.head()?;
    out.current_branch = head.referent_name().map(|n| n.shorten().to_string());
    out.head = repo.head_id().ok().map(|id| id.detach());
    if let Some(id) = out.head {
        out.refs.push(Ref { kind: RefKind::Head, name: "HEAD".into(), full_name: "HEAD".into(), target: id });
    }
    Ok(out)
}

fn is_commit(repo: &gix::Repository, id: ObjectId) -> bool {
    repo.find_header(id).map(|h| h.kind() == gix::object::Kind::Commit).unwrap_or(false)
}
