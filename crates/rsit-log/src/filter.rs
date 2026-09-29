//! Log filters (IntelliJ `VcsLogFilterCollection`): branch, text, user, paths.
//! Branch filters are resolved on the graph, text and user filters scan commit
//! objects in parallel, path filters run `git log -- <paths>`.

use std::collections::HashSet;

use anyhow::{Context as _, Result};
use gix::objs::FindExt as _;
use rayon::prelude::*;
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
    let text = filter.text.trim();
    let user = filter.user.trim();
    if !text.is_empty() || !user.is_empty() {
        let matcher = Matcher::new(text, filter.regex, filter.match_case, user)?;
        scan(data, &matcher, &mut visible);
    }
    if !filter.paths.is_empty() {
        let touching = commits_touching(data, filter)?;
        for (row, v) in visible.iter_mut().enumerate() {
            *v = *v && touching.contains(&data.id(row as u32));
        }
    }
    Ok(Some(visible))
}

pub fn filtered_graph(data: &LogData, filter: &LogFilter) -> Result<Option<FilteredGraph>> {
    Ok(visible_rows(data, filter)?.map(|visible| FilteredGraph::new(&data.graph.linear, &visible)))
}

struct Matcher {
    text: Option<TextMatch>,
    /// Lowercased user substring.
    user: Option<String>,
    /// Lowercased hex prefix: IntelliJ's text filter also matches hashes.
    hash_prefix: Option<String>,
}

enum TextMatch {
    Regex(regex::Regex),
    Plain { needle: String, match_case: bool },
}

impl Matcher {
    fn new(text: &str, regex: bool, match_case: bool, user: &str) -> Result<Self> {
        let text_match = if text.is_empty() {
            None
        } else if regex {
            let re = regex::RegexBuilder::new(text)
                .case_insensitive(!match_case)
                // git --grep matches per line: ^ and $ are line boundaries
                .multi_line(true)
                .build()
                .with_context(|| format!("invalid regex {text:?}"))?;
            Some(TextMatch::Regex(re))
        } else {
            let needle = if match_case { text.to_string() } else { text.to_lowercase() };
            Some(TextMatch::Plain { needle, match_case })
        };
        let hash_prefix =
            (text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase());
        Ok(Self { text: text_match, user: (!user.is_empty()).then(|| user.to_lowercase()), hash_prefix })
    }

    fn matches(&self, id: &ObjectId, commit: &gix::objs::CommitRef<'_>) -> bool {
        if let Some(user) = &self.user {
            let matches_sig = |sig: gix::actor::SignatureRef<'_>| {
                sig.name.to_string().to_lowercase().contains(user)
                    || sig.email.to_string().to_lowercase().contains(user)
            };
            // like IntelliJ's VcsLogUserFilterImpl: the author only
            if !commit.author().map(matches_sig).unwrap_or(false) {
                return false;
            }
        }
        let Some(text) = &self.text else { return true };
        if self.hash_prefix.as_ref().is_some_and(|p| id.to_hex().to_string().starts_with(p.as_str())) {
            return true;
        }
        let message = String::from_utf8_lossy(commit.message);
        match text {
            TextMatch::Regex(re) => re.is_match(&message),
            TextMatch::Plain { needle, match_case: true } => message.contains(needle.as_str()),
            TextMatch::Plain { needle, match_case: false } => message.to_lowercase().contains(needle.as_str()),
        }
    }
}

/// Clears `visible[row]` for commits that do not match, reading commits in parallel.
fn scan(data: &LogData, matcher: &Matcher, visible: &mut [bool]) {
    let repo = &data.repo;
    visible.par_iter_mut().enumerate().with_min_len(4096).for_each_init(
        || (repo.local(), Vec::new()),
        |(local, buf), (row, v)| {
            if !*v {
                return;
            }
            let id = data.id(row as u32);
            *v = match local.objects.find_commit(&id, buf) {
                Ok(commit) => matcher.matches(&id, &commit),
                Err(_) => false,
            };
        },
    );
}

/// Commits that change any of `filter.paths` (git history simplification), within the
/// filtered branches.
fn commits_touching(data: &LogData, filter: &LogFilter) -> Result<HashSet<ObjectId>> {
    let mut args: Vec<String> = vec!["log".into(), "--format=%H".into()];
    if filter.branches.is_empty() {
        args.extend(["--branches", "--remotes", "--tags", "HEAD"].map(String::from));
    } else {
        for name in &filter.branches {
            let full = data.refs.refs.iter().find(|r| &r.name == name).map(|r| match r.kind {
                RefKind::Head => "HEAD".to_string(),
                _ => r.full_name.clone(),
            });
            args.extend(full);
        }
    }
    args.push("--".into());
    args.extend(filter.paths.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = rsit_git::cli::run(data.repo.cwd(), &arg_refs)?;
    Ok(out.lines().filter_map(|l| ObjectId::from_hex(l.trim().as_bytes()).ok()).collect())
}
