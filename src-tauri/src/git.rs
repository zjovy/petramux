//! Snapshots of a working tree as git tree objects, for "what changed this turn".
use crate::shellenv::run_checked;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const GIT: &str = "/usr/bin/git";
const SHORT: Duration = Duration::from_secs(1);
const LIMIT: Duration = Duration::from_secs(3);
/// git's empty tree: the base of "all changes" before the first commit.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const MAX_DIFF: usize = 1 << 20;
/// Untracked files bigger than this stay out of snapshots (and so out of turn diffs).
const MAX_UNTRACKED: u64 = 5 << 20;

fn git(dir: &Path, args: &[&str], index: Option<&Path>, limit: Duration) -> Option<String> {
    let mut cmd = Command::new(GIT);
    cmd.arg("-C").arg(dir).args(args).env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    run_checked(cmd, limit).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Removes the temporary index (and a lock git may leave) however the snapshot ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}

/// The folder's whole repo as a tree id: tracked changes, untracked files and deletions.
/// Works on a copy of the index, so the user's staging and files are untouched.
pub fn snapshot(dir: &Path) -> Option<String> {
    let top = git(dir, &["rev-parse", "--show-toplevel"], None, SHORT)?;
    let top = Path::new(top.trim());
    let index = git(top, &["rev-parse", "--path-format=absolute", "--git-path", "index"], None, SHORT)?;
    // Hashing a big untracked file copies it into .git on every prompt (and a killed
    // `git add` leaves a partial pack behind): leave such files out of snapshots.
    let untracked = git(top, &["ls-files", "-z", "--others", "--exclude-standard"], None, LIMIT)?;
    let big: Vec<String> = untracked
        .split('\0')
        .filter(|p| !p.is_empty() && std::fs::symlink_metadata(top.join(p)).is_ok_and(|m| m.len() > MAX_UNTRACKED))
        .map(|p| format!(":(exclude,literal){p}"))
        .collect();
    if big.len() > 500 {
        return None; // ponytail: no turn diff in folders full of big untracked files
    }
    let tmp = Scratch(std::env::temp_dir().join(format!("roost-index-{}", uuid::Uuid::new_v4())));
    let index = Path::new(index.trim());
    if index.exists() {
        std::fs::copy(index, &tmp.0).ok()?; // keeps git's stat cache: only changed files are hashed
    }
    let mut add = vec!["add", "-A", "--", "."];
    add.extend(big.iter().map(String::as_str));
    git(top, &add, Some(&tmp.0), LIMIT)?;
    let tree = git(top, &["write-tree"], Some(&tmp.0), SHORT)?;
    Some(tree.trim().to_string()).filter(|t| !t.is_empty())
}

/// The repo a folder is in: its top folder.
pub fn toplevel(dir: &Path) -> Option<String> {
    Some(git(dir, &["rev-parse", "--show-toplevel"], None, SHORT)?.trim().to_string()).filter(|t| !t.is_empty())
}

/// The branch a folder is on (the commit, when detached); None outside a repo.
pub fn branch(dir: &Path) -> Option<String> {
    let name = git(dir, &["symbolic-ref", "-q", "--short", "HEAD"], None, SHORT)
        .or_else(|| git(dir, &["rev-parse", "--short", "HEAD"], None, SHORT))?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

/// `git diff` from `base` (a tree id; HEAD's tree when None) to the folder as it is now.
pub fn diff(dir: &Path, base: Option<&str>) -> Result<String, String> {
    if !dir.is_dir() {
        return Err("Couldn't read the diff: the folder is gone.".into());
    }
    if git(dir, &["rev-parse", "--is-inside-work-tree"], None, SHORT).is_none() {
        return Err("Not a git repository.".into());
    }
    let base = match base {
        Some(b) => b.to_string(),
        None => git(dir, &["rev-parse", "--verify", "-q", "HEAD^{tree}"], None, SHORT)
            .map(|t| t.trim().to_string())
            .unwrap_or_else(|| EMPTY_TREE.into()),
    };
    let now = snapshot(dir).ok_or("Couldn't read the diff: git took too long or failed.")?;
    let out = git(dir, &["diff", "--no-color", "--no-ext-diff", "-M", &base, &now], None, LIMIT).ok_or_else(|| {
        let tree = format!("{base}^{{tree}}");
        if git(dir, &["cat-file", "-e", &tree], None, SHORT).is_none() {
            "Couldn't read the diff: the turn's snapshot is gone.".to_string()
        } else {
            "Couldn't read the diff: git took too long.".to_string()
        }
    })?;
    Ok(truncate(out))
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_DIFF {
        let mut cut = MAX_DIFF;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… diff truncated\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new(GIT)
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        sh(d.path(), &["init", "-q"]);
        fs::write(d.path().join("a.txt"), "one\n").unwrap();
        sh(d.path(), &["add", "a.txt"]);
        sh(d.path(), &["commit", "-qm", "x"]);
        d
    }

    #[test]
    fn the_branch_or_the_commit_a_folder_is_on() {
        let d = repo();
        sh(d.path(), &["checkout", "-qb", "feat/x"]);
        assert_eq!(branch(d.path()).as_deref(), Some("feat/x"));
        sh(d.path(), &["checkout", "-q", "--detach"]);
        let head = sh(d.path(), &["rev-parse", "--short", "HEAD"]);
        assert_eq!(branch(d.path()).as_deref(), Some(head.trim()), "detached: the commit");
        assert_eq!(branch(tempfile::tempdir().unwrap().path()), None, "not a repo");
        let empty = tempfile::tempdir().unwrap();
        sh(empty.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(branch(empty.path()).as_deref(), Some("main"), "no commits yet");
    }

    #[test]
    fn snapshot_captures_untracked_files_and_leaves_the_index_alone() {
        let d = repo();
        let index = fs::read(d.path().join(".git/index")).unwrap();
        let base = snapshot(d.path()).unwrap();
        fs::write(d.path().join("new file.txt"), "hi\n").unwrap();
        fs::write(d.path().join("a.txt"), "two\n").unwrap();
        let out = diff(d.path(), Some(&base)).unwrap();
        assert!(out.contains("+++ b/new file.txt") && out.contains("+two"), "{out}");
        assert_eq!(fs::read(d.path().join(".git/index")).unwrap(), index);
        assert_eq!(sh(d.path(), &["status", "--porcelain"]), " M a.txt\n?? \"new file.txt\"\n");
    }

    #[test]
    fn an_unchanged_folder_has_an_empty_diff() {
        let d = repo();
        let base = snapshot(d.path()).unwrap();
        assert_eq!(diff(d.path(), Some(&base)).unwrap(), "");
    }

    #[test]
    fn all_changes_work_without_commits() {
        let d = tempfile::tempdir().unwrap();
        sh(d.path(), &["init", "-q"]);
        fs::write(d.path().join("b.txt"), "x\n").unwrap();
        assert!(diff(d.path(), None).unwrap().contains("+++ b/b.txt"));
    }

    #[test]
    fn outside_a_repo_there_is_no_snapshot_and_a_plain_error() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(snapshot(d.path()), None);
        assert_eq!(diff(d.path(), None), Err("Not a git repository.".into()));
        assert!(diff(Path::new("/no/such/dir"), None).unwrap_err().starts_with("Couldn't read the diff"));
    }

    #[test]
    fn big_untracked_files_stay_out_of_git() {
        let d = repo();
        // Incompressible bytes, bigger than the cap.
        let mut x: u64 = 7;
        let big: Vec<u8> = (0..(6 << 20))
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (x >> 56) as u8
            })
            .collect();
        fs::write(d.path().join("big.bin"), &big).unwrap();
        fs::write(d.path().join("small.txt"), "hi\n").unwrap();
        let blob = sh(d.path(), &["hash-object", "big.bin"]);
        let base = snapshot(d.path()).expect("a snapshot despite the big file");
        let missing = Command::new(GIT).arg("-C").arg(d.path()).args(["cat-file", "-e", blob.trim()]).status().unwrap();
        assert!(!missing.success(), "big.bin was copied into .git");
        fs::write(d.path().join("small.txt"), "bye\n").unwrap();
        let out = diff(d.path(), Some(&base)).unwrap();
        assert!(out.contains("+bye") && !out.contains("big.bin"), "{out}");
    }

    #[test]
    fn a_vanished_snapshot_says_so() {
        let d = repo();
        let gone = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(diff(d.path(), Some(gone)), Err("Couldn't read the diff: the turn's snapshot is gone.".into()));
    }


    #[test]
    fn big_diffs_are_cut() {
        let cut = truncate("é".repeat(MAX_DIFF));
        assert!(cut.len() < MAX_DIFF + 40 && cut.ends_with("… diff truncated\n"));
    }
}
