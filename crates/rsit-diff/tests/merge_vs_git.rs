//! Differential test: when `git merge-file` merges cleanly, applying all
//! non-conflicting chunks must give the same text.

use std::process::Command;

use rsit_diff::WhitespacePolicy;
use rsit_diff::merge::MergeModel;

/// Tiny deterministic PRNG (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Random edits (replace, insert, delete) in a random window of the base.
fn edit(base: &[String], rng: &mut Rng, tag: &str) -> Vec<String> {
    let mut lines = base.to_vec();
    for _ in 0..1 + rng.below(3) {
        let at = rng.below(lines.len() as u64 + 1) as usize;
        match rng.below(3) {
            0 if at < lines.len() => lines[at] = format!("{tag} changed {}", rng.below(1000)),
            1 => lines.insert(at, format!("{tag} inserted {}", rng.below(1000))),
            _ if at < lines.len() => {
                lines.remove(at);
            }
            _ => {}
        }
    }
    lines
}

fn text(lines: &[String]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

#[test]
fn clean_merges_agree_with_git() {
    let dir = tempfile::tempdir().unwrap();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let (mut clean, mut compared) = (0, 0);
    for case in 0..300 {
        let base: Vec<String> = (0..10 + rng.below(30)).map(|i| format!("line {i}")).collect();
        let (left, right) = (edit(&base, &mut rng, "L"), edit(&base, &mut rng, "R"));
        let (bt, lt, rt) = (text(&base), text(&left), text(&right));
        let path = |n: &str| dir.path().join(format!("{case}-{n}"));
        std::fs::write(path("base"), &bt).unwrap();
        std::fs::write(path("left"), &lt).unwrap();
        std::fs::write(path("right"), &rt).unwrap();
        let out = Command::new("git")
            .arg("merge-file")
            .arg("-p")
            .arg(path("left"))
            .arg(path("base"))
            .arg(path("right"))
            .output()
            .unwrap();
        let git_conflicts = out.status.code().unwrap_or(-1);

        let mut model = MergeModel::new(bt.clone(), lt.clone(), rt.clone(), WhitespacePolicy::Default);
        if git_conflicts == 0 {
            clean += 1;
            // git found no conflict; we may still be more conservative (adjacent changes)
            if model.unresolved_conflicts() == 0 {
                compared += 1;
                model.apply_non_conflicting();
                assert_eq!(
                    model.result(),
                    String::from_utf8_lossy(&out.stdout),
                    "case {case}:\nbase:\n{bt}\nleft:\n{lt}\nright:\n{rt}"
                );
            }
        } else {
            assert!(
                model.unresolved_conflicts() > 0,
                "case {case}: git conflicts but we do not\nbase:\n{bt}\nleft:\n{lt}\nright:\n{rt}"
            );
        }
    }
    assert!(compared * 10 >= clean * 8, "compared {compared} of {clean} clean merges");
}
