use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use anyhow::Result;
use gix::ObjectId;
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;

/// Every commit reachable from the given tips, in `git log --date-order` order:
/// children before parents, otherwise newest commit time first.
#[derive(Default)]
pub struct CommitGraphData {
    /// Commit ids in display order.
    pub ids: Vec<ObjectId>,
    /// Parent indices into `ids`; `u32::MAX` marks a parent that is not loaded
    /// (shallow clone or a partial walk).
    pub parents: Vec<Vec<u32>>,
    pub times: Vec<i64>,
    pub index: HashMap<ObjectId, u32>,
}

impl CommitGraphData {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// Walks the history of `tips`. `limit` stops the walk after that many commits
/// (used for the fast first screen).
pub fn load_commit_graph(repo: &gix::Repository, tips: &[ObjectId], limit: Option<usize>) -> Result<CommitGraphData> {
    struct Raw {
        id: ObjectId,
        parents: Vec<ObjectId>,
        time: i64,
    }
    let mut raw: Vec<Raw> = Vec::new();
    if !tips.is_empty() {
        let walk = repo
            .rev_walk(tips.iter().copied())
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .use_commit_graph(true)
            .all()?;
        for info in walk {
            let info = info?;
            raw.push(Raw {
                id: info.id,
                parents: info.parent_ids.iter().copied().collect(),
                time: info.commit_time.unwrap_or_default(),
            });
            if limit.is_some_and(|l| raw.len() >= l) {
                break;
            }
        }
    }

    // walk position -> index; parents missing from the walk are "not loaded"
    let pos: HashMap<ObjectId, u32> = raw.iter().enumerate().map(|(i, r)| (r.id, i as u32)).collect();
    let parents: Vec<Vec<u32>> =
        raw.iter().map(|r| r.parents.iter().map(|p| pos.get(p).copied().unwrap_or(u32::MAX)).collect()).collect();

    // Kahn's algorithm with a max-heap on commit time, ties in walk order
    // (git's `sort_in_topological_order` with REV_SORT_BY_COMMIT_DATE).
    let n = raw.len();
    let mut children = vec![0u32; n];
    for ps in &parents {
        for &p in ps {
            if p != u32::MAX {
                children[p as usize] += 1;
            }
        }
    }
    // ties are broken by insertion order, like git's prio_queue
    let mut seq = 0u32;
    let mut heap: BinaryHeap<(i64, Reverse<u32>, u32)> = BinaryHeap::with_capacity(n);
    for i in 0..n as u32 {
        if children[i as usize] == 0 {
            heap.push((raw[i as usize].time, Reverse(seq), i));
            seq += 1;
        }
    }
    let mut order: Vec<u32> = Vec::with_capacity(n);
    while let Some((_, _, i)) = heap.pop() {
        order.push(i);
        for &p in &parents[i as usize] {
            if p == u32::MAX {
                continue;
            }
            let c = &mut children[p as usize];
            *c -= 1;
            if *c == 0 {
                heap.push((raw[p as usize].time, Reverse(seq), p));
                seq += 1;
            }
        }
    }

    let mut new_index = vec![0u32; n];
    for (new, &old) in order.iter().enumerate() {
        new_index[old as usize] = new as u32;
    }
    let mut out = CommitGraphData {
        ids: Vec::with_capacity(n),
        parents: Vec::with_capacity(n),
        times: Vec::with_capacity(n),
        index: HashMap::with_capacity(n),
    };
    for &old in &order {
        let r = &raw[old as usize];
        out.index.insert(r.id, out.ids.len() as u32);
        out.ids.push(r.id);
        out.times.push(r.time);
        out.parents.push(
            parents[old as usize].iter().map(|&p| if p == u32::MAX { u32::MAX } else { new_index[p as usize] }).collect(),
        );
    }
    Ok(out)
}
