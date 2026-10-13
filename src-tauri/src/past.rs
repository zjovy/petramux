use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Past {
    pub claude_id: String,
    pub title: String,
    pub workdir: String,
    pub modified_ms: u64,
}

pub fn encode_path(p: &str) -> String {
    p.replace(['/', '.'], "-")
}

/// Claude saves a conversation only once it has a prompt: a Claude never prompted has nothing to resume.
pub fn saved(projects: &Path, claude_id: &str) -> bool {
    let file = format!("{claude_id}.jsonl");
    fs::read_dir(projects).into_iter().flatten().flatten().any(|d| d.path().join(&file).is_file())
}

pub fn read_meta(path: &Path) -> Option<(String, String)> {
    let file = fs::File::open(path).ok()?;
    let (mut custom, mut ai, mut first, mut cwd) = (None, None, None, None);
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(String::from),
            Some("ai-title") => ai = v["aiTitle"].as_str().map(String::from),
            Some("user") if first.is_none() && v["isMeta"].as_bool() != Some(true) => {
                first = user_text(&v["message"]["content"]).filter(|t| !t.trim_start().starts_with('<'));
            }
            _ => {}
        }
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(String::from);
        }
    }
    let title = custom.or(ai).or(first)?;
    Some((one_line(&title, 80), cwd?))
}

fn user_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => parts
            .iter()
            .find(|p| p["type"] == "text")
            .and_then(|p| p["text"].as_str().map(String::from)),
        _ => None,
    }
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        flat.chars().take(max - 1).collect::<String>() + "…"
    }
}

pub fn list_past(projects: &Path, repo: &str, exclude: &HashSet<String>) -> Vec<Past> {
    let enc = encode_path(repo);
    let worktrees = format!("{enc}--claude-worktrees-");
    let mut files: Vec<(u64, PathBuf)> = vec![];
    for dir in fs::read_dir(projects).into_iter().flatten().flatten() {
        let name = dir.file_name().to_string_lossy().into_owned();
        if name != enc && !name.starts_with(&worktrees) {
            continue;
        }
        for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let id = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            if exclude.contains(&id) {
                continue;
            }
            let ms = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            files.push((ms, p));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    // ponytail: reads whole transcripts, capped at the 50 newest; cache titles if ⌘O gets slow.
    files
        .into_iter()
        .take(50)
        .filter_map(|(ms, p)| {
            let (title, workdir) = read_meta(&p)?;
            let claude_id = p.file_stem()?.to_string_lossy().into_owned();
            Some(Past { claude_id, title, workdir, modified_ms: ms })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn write(dir: &Path, sub: &str, id: &str, lines: &[&str], age_s: u64) {
        let d = dir.join(sub);
        fs::create_dir_all(&d).unwrap();
        let p = d.join(format!("{id}.jsonl"));
        fs::write(&p, lines.join("\n")).unwrap();
        let f = fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    #[test]
    fn encodes_like_claude_code() {
        assert_eq!(encode_path("/Users/me/Dev/petra.app"), "-Users-me-Dev-petra-app");
    }

    #[test]
    fn title_prefers_custom_then_ai_then_first_real_prompt() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "-r-app", "a", &[
            r#"{"type":"user","isMeta":true,"message":{"content":"meta"},"cwd":"/r/app"}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"},"cwd":"/r/app"}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"fix   the\nflaky test"}]},"cwd":"/r/app"}"#,
        ], 0);
        write(dir.path(), "-r-app", "b", &[
            r#"{"type":"user","message":{"content":"hello"},"cwd":"/r/app"}"#,
            r#"{"type":"ai-title","aiTitle":"AI name"}"#,
            r#"{"type":"custom-title","customTitle":"Mine"}"#,
        ], 0);
        assert_eq!(read_meta(&dir.path().join("-r-app/a.jsonl")).unwrap(), ("fix the flaky test".into(), "/r/app".into()));
        assert_eq!(read_meta(&dir.path().join("-r-app/b.jsonl")).unwrap().0, "Mine");
    }

    #[test]
    fn long_titles_are_cut_to_80() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!(r#"{{"type":"user","message":{{"content":"{}"}},"cwd":"/r"}}"#, "x".repeat(200));
        write(dir.path(), "-r", "a", &[&long], 0);
        let (t, _) = read_meta(&dir.path().join("-r/a.jsonl")).unwrap();
        assert_eq!(t.chars().count(), 80);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn lists_repo_and_its_worktrees_newest_first_excluding_open_ones() {
        let dir = tempfile::tempdir().unwrap();
        let user = |cwd: &str| format!(r#"{{"type":"user","message":{{"content":"hi"}},"cwd":"{cwd}"}}"#);
        write(dir.path(), "-r-app", "old", &[&user("/r/app")], 100);
        write(dir.path(), "-r-app--claude-worktrees-x", "wt", &[&user("/r/app/.claude/worktrees/x")], 10);
        write(dir.path(), "-r-app", "open", &[&user("/r/app")], 5);
        write(dir.path(), "-r-app-other", "sibling", &[&user("/r/app-other")], 1);
        let exclude: HashSet<String> = ["open".to_string()].into();
        let got = list_past(dir.path(), "/r/app", &exclude);
        let ids: Vec<&str> = got.iter().map(|p| p.claude_id.as_str()).collect();
        assert_eq!(ids, vec!["wt", "old"]);
        assert_eq!(got[0].workdir, "/r/app/.claude/worktrees/x");
    }

    #[test]
    fn a_conversation_is_saved_only_if_its_file_exists_in_some_project() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "-r-app-.claude-worktrees-x", "talked", &["{}"], 0);
        assert!(saved(dir.path(), "talked"));
        assert!(!saved(dir.path(), "never-prompted"));
        assert!(!saved(&dir.path().join("missing"), "talked"));
    }
}
