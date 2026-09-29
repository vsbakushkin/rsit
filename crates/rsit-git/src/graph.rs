use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use anyhow::Result;
use gix::ObjectId;
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;

/// Parent index of a commit that is not loaded (shallow clone or partial walk).
pub const MISSING: u32 = u32::MAX;

/// Every commit reachable from the given tips, in `git log --date-order` order:
/// children before parents, otherwise newest commit time first.
#[derive(Default)]
pub struct CommitGraphData {
    /// Commit ids in display order.
    pub ids: Vec<ObjectId>,
    pub times: Vec<i64>,
    /// `parent_list[parent_offsets[i]..parent_offsets[i + 1]]` are the parents of `i`.
    parent_offsets: Vec<u32>,
    parent_list: Vec<u32>,
    /// Row indices sorted by id, for [`CommitGraphData::row_of`] (much smaller than a hash map
    /// on millions of commits).
    by_id: Vec<u32>,
}

impl CommitGraphData {
    /// Builds from raw arrays (e.g. read from a cache); parents are indices, `MISSING` allowed.
    pub fn from_parts(ids: Vec<ObjectId>, times: Vec<i64>, parent_offsets: Vec<u32>, parent_list: Vec<u32>) -> Self {
        assert_eq!(ids.len(), times.len());
        assert_eq!(parent_offsets.len(), ids.len() + 1);
        let mut by_id: Vec<u32> = (0..ids.len() as u32).collect();
        by_id.sort_unstable_by(|&a, &b| ids[a as usize].cmp(&ids[b as usize]));
        Self { ids, times, parent_offsets, parent_list, by_id }
    }

    /// Row of the commit `id`, if it is loaded.
    pub fn row_of(&self, id: &gix::oid) -> Option<u32> {
        self.by_id.binary_search_by(|&i| self.ids[i as usize].as_ref().cmp(id)).ok().map(|pos| self.by_id[pos])
    }

    pub fn contains(&self, id: &gix::oid) -> bool {
        self.row_of(id).is_some()
    }

    /// Like [`CommitGraphData::from_parts`] with a precomputed `by_id` index
    /// (see [`CommitGraphData::parts`]); the index is checked for consistency.
    pub fn from_parts_indexed(
        ids: Vec<ObjectId>,
        times: Vec<i64>,
        parent_offsets: Vec<u32>,
        parent_list: Vec<u32>,
        by_id: Vec<u32>,
    ) -> Option<Self> {
        let n = ids.len();
        if times.len() != n || parent_offsets.len() != n + 1 || by_id.len() != n {
            return None;
        }
        let sorted = by_id.iter().all(|&i| (i as usize) < n)
            && by_id.windows(2).all(|w| ids[w[0] as usize] < ids[w[1] as usize]);
        sorted.then_some(Self { ids, times, parent_offsets, parent_list, by_id })
    }

    /// Raw arrays: ids, times, parent offsets, parent list, rows sorted by id.
    pub fn parts(&self) -> (&[ObjectId], &[i64], &[u32], &[u32], &[u32]) {
        (&self.ids, &self.times, &self.parent_offsets, &self.parent_list, &self.by_id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Parent indices of commit `i` (`MISSING` for parents that are not loaded).
    pub fn parents(&self, i: u32) -> &[u32] {
        &self.parent_list[self.parent_offsets[i as usize] as usize..self.parent_offsets[i as usize + 1] as usize]
    }
}

/// Commits in arbitrary order with parents as indices into the same arrays.
struct Unsorted {
    ids: Vec<ObjectId>,
    times: Vec<i64>,
    offsets: Vec<u32>,
    list: Vec<u32>,
}

impl Unsorted {
    fn parents(&self, i: usize) -> &[u32] {
        &self.list[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }
}

/// Walks the history of `tips`. `limit` stops the walk after that many commits
/// (used for the fast first screen). Commits in `cache` are taken from it
/// instead of being read again; only newer history is walked.
pub fn load_commit_graph(
    repo: &gix::Repository,
    tips: &[ObjectId],
    limit: Option<usize>,
    cache: Option<CommitGraphData>,
) -> Result<CommitGraphData> {
    let cache_owned = cache;
    let cache = cache_owned.as_ref();
    let mut new_ids: Vec<ObjectId> = Vec::new();
    let mut new_times: Vec<i64> = Vec::new();
    let mut new_parents: Vec<Vec<ObjectId>> = Vec::new();
    let tips_to_walk: Vec<ObjectId> = tips.iter().copied().filter(|t| cache.is_none_or(|c| !c.contains(t))).collect();
    if !tips_to_walk.is_empty() {
        let walk = repo
            .rev_walk(tips_to_walk)
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .use_commit_graph(true)
            .selected(move |id| cache.is_none_or(|c| !c.contains(id)))?;
        for info in walk {
            let info = info?;
            new_ids.push(info.id);
            new_times.push(info.commit_time.unwrap_or_default());
            new_parents.push(info.parent_ids.iter().copied().collect());
            if limit.is_some_and(|l| new_ids.len() >= l) {
                break;
            }
        }
    }

    // nothing new and every cached commit still reachable: the cache is already in display order
    if new_ids.is_empty() {
        if let Some(c) = cache {
            let mut reachable = vec![false; c.len()];
            let mut stack: Vec<u32> = tips.iter().filter_map(|t| c.row_of(t)).collect();
            for &s in &stack {
                reachable[s as usize] = true;
            }
            while let Some(i) = stack.pop() {
                for &p in c.parents(i) {
                    if p != MISSING && !reachable[p as usize] {
                        reachable[p as usize] = true;
                        stack.push(p);
                    }
                }
            }
            if reachable.iter().all(|&r| r) {
                return Ok(cache_owned.expect("cache present"));
            }
        }
    }

    // combined index space: new commits first, then the cached ones
    let k = new_ids.len() as u32;
    let new_pos: HashMap<ObjectId, u32> = new_ids.iter().enumerate().map(|(i, id)| (*id, i as u32)).collect();
    let lookup = |id: &ObjectId| -> u32 {
        new_pos.get(id).copied().or_else(|| cache.and_then(|c| c.row_of(id)).map(|i| i + k)).unwrap_or(MISSING)
    };
    let cached_len = cache.map_or(0, |c| c.len());
    let n = k as usize + cached_len;
    let mut all = Unsorted {
        ids: Vec::with_capacity(n),
        times: Vec::with_capacity(n),
        offsets: Vec::with_capacity(n + 1),
        list: Vec::with_capacity(n + n / 8),
    };
    all.offsets.push(0);
    for (i, parents) in new_parents.iter().enumerate() {
        all.ids.push(new_ids[i]);
        all.times.push(new_times[i]);
        all.list.extend(parents.iter().map(&lookup));
        all.offsets.push(all.list.len() as u32);
    }
    if let Some(c) = cache {
        all.ids.extend_from_slice(&c.ids);
        all.times.extend_from_slice(&c.times);
        for i in 0..c.len() as u32 {
            all.list.extend(c.parents(i).iter().map(|&p| if p == MISSING { MISSING } else { p + k }));
            all.offsets.push(all.list.len() as u32);
        }
    }

    // only commits reachable from the current tips (branches may have been
    // deleted or rewritten since the cache was written)
    let reachable = if cache.is_some() {
        let mut reachable = vec![false; n];
        let mut stack: Vec<u32> = tips.iter().map(&lookup).filter(|&i| i != MISSING).collect();
        for &s in &stack {
            reachable[s as usize] = true;
        }
        while let Some(i) = stack.pop() {
            for &p in all.parents(i as usize) {
                if p != MISSING && !reachable[p as usize] {
                    reachable[p as usize] = true;
                    stack.push(p);
                }
            }
        }
        Some(reachable)
    } else {
        None
    };
    Ok(date_order(&all, reachable.as_deref()))
}

/// Kahn's algorithm with a max-heap on commit time; ties are broken by insertion
/// order like git's prio_queue (`sort_in_topological_order` with
/// REV_SORT_BY_COMMIT_DATE), which reproduces `git log --date-order`.
fn date_order(all: &Unsorted, keep: Option<&[bool]>) -> CommitGraphData {
    let n = all.ids.len();
    let kept = |i: usize| keep.is_none_or(|k| k[i]);
    let mut children = vec![0u32; n];
    for i in (0..n).filter(|&i| kept(i)) {
        for &p in all.parents(i) {
            if p != MISSING {
                children[p as usize] += 1;
            }
        }
    }
    let mut seq = 0u32;
    let mut heap: BinaryHeap<(i64, Reverse<u32>, u32)> = BinaryHeap::with_capacity(1024);
    for i in 0..n {
        if kept(i) && children[i] == 0 {
            heap.push((all.times[i], Reverse(seq), i as u32));
            seq += 1;
        }
    }
    let mut order: Vec<u32> = Vec::with_capacity(n);
    while let Some((_, _, i)) = heap.pop() {
        order.push(i);
        for &p in all.parents(i as usize) {
            if p == MISSING {
                continue;
            }
            let c = &mut children[p as usize];
            *c -= 1;
            if *c == 0 {
                heap.push((all.times[p as usize], Reverse(seq), p));
                seq += 1;
            }
        }
    }

    let mut new_index = vec![MISSING; n];
    for (new, &old) in order.iter().enumerate() {
        new_index[old as usize] = new as u32;
    }
    let mut ids = Vec::with_capacity(order.len());
    let mut times = Vec::with_capacity(order.len());
    let mut offsets = Vec::with_capacity(order.len() + 1);
    let mut list = Vec::with_capacity(all.list.len());
    offsets.push(0);
    for &old in &order {
        ids.push(all.ids[old as usize]);
        times.push(all.times[old as usize]);
        list.extend(all.parents(old as usize).iter().map(
            |&p| {
                if p == MISSING { MISSING } else { new_index[p as usize] }
            },
        ));
        offsets.push(list.len() as u32);
    }
    CommitGraphData::from_parts(ids, times, offsets, list)
}
