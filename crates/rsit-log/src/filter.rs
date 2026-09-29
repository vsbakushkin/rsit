//! Log filters (IntelliJ `VcsLogFilterCollection`): branch, text, user, paths.
//! Branch filters are resolved on the graph; text/user/path filters run
//! `git log` until the persistent index exists.

use std::collections::HashSet;

use anyhow::Result;
use rsit_git::{ObjectId, RefKind};
use rsit_graph::FilteredGraph;

use crate::LogData;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    pub text: String,
    pub regex: bool,
    pub match_case: bool,
    /// Author name or email (substring, case-insensitive).
    pub user: String,
    /// Short ref names (`main`, `origin/main`, `v1.0`, `HEAD`).
    pub branches: Vec<String>,
    /// Repository-relative paths.
    pub paths: Vec<String>,
}

impl LogFilter {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.user.trim().is_empty() && self.branches.is_empty() && self.paths.is_empty()
    }

    fn needs_matching(&self) -> bool {
        !self.text.trim().is_empty() || !self.user.trim().is_empty() || !self.paths.is_empty()
    }
}

/// Computes which rows of `data` stay visible under `filter`; `None` means all.
pub fn visible_rows(data: &LogData, filter: &LogFilter) -> Result<Option<Vec<bool>>> {
    if filter.is_empty() {
        return Ok(None);
    }
    let mut visible = if filter.branches.is_empty() {
        vec![true; data.len()]
    } else {
        let heads = filter.branches.iter().flat_map(|name| {
            data.refs.refs.iter().filter(move |r| &r.name == name).filter_map(|r| data.row_of(&r.target))
        });
        data.graph.reachable_from(heads)
    };
    if filter.needs_matching() {
        let matched = matching_commits(data, filter)?;
        for (row, v) in visible.iter_mut().enumerate() {
            *v = *v && matched.contains(&data.id(row as u32));
        }
    }
    Ok(Some(visible))
}

pub fn filtered_graph(data: &LogData, filter: &LogFilter) -> Result<Option<FilteredGraph>> {
    Ok(visible_rows(data, filter)?.map(|visible| FilteredGraph::new(&data.graph.linear, &visible)))
}

fn matching_commits(data: &LogData, filter: &LogFilter) -> Result<HashSet<ObjectId>> {
    let mut base: Vec<String> = vec!["log".into(), "--format=%H".into()];
    if filter.branches.is_empty() {
        base.extend(["--branches", "--remotes", "--tags", "HEAD"].map(String::from));
    } else {
        for name in &filter.branches {
            let full = data.refs.refs.iter().find(|r| &r.name == name).map(|r| match r.kind {
                RefKind::Head => "HEAD".to_string(),
                _ => r.full_name.clone(),
            });
            base.extend(full);
        }
    }
    let text = filter.text.trim();
    let user = filter.user.trim();

    // each pattern kind gets its own `git log` so regex modes do not interfere
    let mut queries: Vec<Vec<String>> = Vec::new();
    if !text.is_empty() {
        let mut q = vec![format!("--grep={text}")];
        if !filter.match_case {
            q.push("--regexp-ignore-case".into());
        }
        q.push(if filter.regex { "--extended-regexp" } else { "--fixed-strings" }.into());
        queries.push(q);
    }
    if !user.is_empty() {
        // IntelliJ matches users as a case-insensitive substring of name or email
        queries.push(vec![format!("--author={user}"), "--regexp-ignore-case".into(), "--fixed-strings".into()]);
    }
    if queries.is_empty() {
        queries.push(Vec::new()); // paths only
    }

    let mut result: Option<HashSet<ObjectId>> = None;
    for (i, q) in queries.iter().enumerate() {
        let mut args = base.clone();
        args.extend(q.iter().cloned());
        args.push("--".into());
        args.extend(filter.paths.iter().cloned());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = rsit_git::cli::run(data.repo.cwd(), &arg_refs)?;
        let mut matched: HashSet<ObjectId> =
            out.lines().filter_map(|l| ObjectId::from_hex(l.trim().as_bytes()).ok()).collect();
        // IntelliJ's text filter also matches commit hashes
        if i == 0 && !text.is_empty() && text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit()) {
            let prefix = text.to_lowercase();
            matched.extend(data.commits.ids.iter().filter(|id| id.to_hex().to_string().starts_with(&prefix)).copied());
        }
        result = Some(match result {
            None => matched,
            Some(prev) => prev.intersection(&matched).copied().collect(),
        });
    }
    Ok(result.unwrap_or_default())
}
