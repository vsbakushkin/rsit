//! Log model: loaded commit graph, refs and lazily read commit metadata.
//! UI-free so it can be tested and benchmarked on its own.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use rsit_git::{CommitGraphData, CommitMeta, ObjectId, Ref, RefKind, Refs, Repo};
use rsit_graph::{GraphCommit, GraphElement, PermanentGraph, PrintElement, PrintElementGenerator};

/// Commits to load for the first screen before the full history is read.
pub const FIRST_SCREEN_COMMITS: usize = 1000;

pub struct LogData {
    pub repo: Repo,
    pub refs: Refs,
    pub commits: CommitGraphData,
    /// Graph over row indices (node index == row == index into `commits`).
    pub graph: PermanentGraph<u32>,
    /// Refs pointing to each row, in label order.
    refs_by_row: HashMap<u32, Vec<Ref>>,
    /// Name of the best ref at each head row, for branch colors.
    head_names: HashMap<u32, String>,
    /// `true` when only the first part of the history is loaded.
    pub partial: bool,
}

impl LogData {
    pub fn load(repo: Repo, limit: Option<usize>) -> Result<Self> {
        let local = repo.local();
        let refs = rsit_git::read_refs(&local)?;
        let commits = rsit_git::load_commit_graph(&local, &refs.tips(), limit)?;
        let partial = limit.is_some_and(|l| commits.len() >= l);
        Ok(Self::from_parts(repo, refs, commits, partial))
    }

    pub fn from_parts(repo: Repo, refs: Refs, commits: CommitGraphData, partial: bool) -> Self {
        let mut refs_by_row: HashMap<u32, Vec<Ref>> = HashMap::new();
        for r in &refs.refs {
            if let Some(&row) = commits.index.get(&r.target) {
                refs_by_row.entry(row).or_default().push(r.clone());
            }
        }
        for list in refs_by_row.values_mut() {
            list.sort_by(|a, b| refs.label_cmp(a, b));
        }

        // not loaded parents get ids past the loaded range so every id is unique
        let n = commits.len() as u32;
        let mut next_missing = n;
        let graph_commits: Vec<GraphCommit<u32>> = commits
            .parents
            .iter()
            .enumerate()
            .map(|(i, ps)| GraphCommit {
                id: i as u32,
                parents: ps
                    .iter()
                    .map(|&p| {
                        if p == u32::MAX {
                            next_missing += 1;
                            next_missing
                        } else {
                            p
                        }
                    })
                    .collect(),
            })
            .collect();

        // IntelliJ: branch heads are the targets of branch refs (tags are not branches)
        let branch_rows: Vec<u32> = refs
            .refs
            .iter()
            .filter(|r| r.kind != RefKind::Tag)
            .filter_map(|r| commits.index.get(&r.target).copied())
            .collect();
        let best_ref = |row: u32| -> Option<&Ref> {
            refs_by_row.get(&row)?.iter().min_by(|a, b| refs.layout_cmp(a, b))
        };
        // HeadCommitsComparator: heads with refs first, by the branch layout comparator
        let head_cmp = |a: &u32, b: &u32| -> Ordering {
            match (best_ref(*a), best_ref(*b)) {
                (Some(ra), Some(rb)) => refs.layout_cmp(ra, rb),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a.cmp(b),
            }
        };
        let graph = PermanentGraph::new(&graph_commits, branch_rows, head_cmp);
        let head_names = graph
            .layout
            .head_nodes()
            .iter()
            .filter_map(|&h| Some((h, best_ref(h)?.name.clone())))
            .collect();

        Self { repo, refs, commits, graph, refs_by_row, head_names, partial }
    }

    pub fn len(&self) -> usize {
        self.commits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    pub fn id(&self, row: u32) -> ObjectId {
        self.commits.ids[row as usize]
    }

    pub fn row_of(&self, id: &ObjectId) -> Option<u32> {
        self.commits.index.get(id).copied()
    }

    pub fn refs_at(&self, row: u32) -> &[Ref] {
        self.refs_by_row.get(&row).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_head(&self, row: u32) -> bool {
        self.refs.head == Some(self.id(row))
    }

    pub fn color_id(&self, element: &GraphElement) -> i32 {
        self.graph.color_id(element, |row| self.head_names.get(row).map(String::as_str))
    }

    /// Rows of the direct parents that are loaded.
    pub fn parent_rows(&self, row: u32) -> impl Iterator<Item = u32> + '_ {
        self.commits.parents[row as usize].iter().copied().filter(|&p| p != u32::MAX)
    }
}

/// Per-view graph drawing state: caches of the print element generator.
pub struct GraphPrinter {
    generator: PrintElementGenerator,
    width: u32,
}

impl GraphPrinter {
    pub fn new(data: &LogData) -> Self {
        let generator = PrintElementGenerator::new(false);
        let width = generator.recommended_width(&data.graph.linear);
        Self { generator, width }
    }

    /// Lanes that fit most rows (IntelliJ's recommended graph width).
    pub fn recommended_width(&self) -> u32 {
        self.width
    }

    pub fn row(&mut self, data: &LogData, row: u32) -> Vec<PrintElement> {
        let li = |n: u32| data.graph.layout.layout_index(n);
        let colors = |e: &GraphElement| data.color_id(e);
        self.generator.print_elements(&data.graph.linear, &li, &colors, row)
    }
}

/// Bounded cache of commit metadata read on demand for visible rows.
pub struct MetaCache {
    repo: gix::Repository,
    entries: HashMap<ObjectId, Arc<CommitMeta>>,
    capacity: usize,
}

impl MetaCache {
    pub fn new(repo: &Repo) -> Self {
        Self { repo: repo.local(), entries: HashMap::new(), capacity: 20_000 }
    }

    pub fn get(&mut self, id: ObjectId) -> Option<Arc<CommitMeta>> {
        if let Some(meta) = self.entries.get(&id) {
            return Some(meta.clone());
        }
        let meta = Arc::new(rsit_git::read_commit(&self.repo, id).ok()?);
        if self.entries.len() >= self.capacity {
            self.entries.clear();
        }
        self.entries.insert(id, meta.clone());
        Some(meta)
    }

    pub fn repo(&self) -> &gix::Repository {
        &self.repo
    }
}
