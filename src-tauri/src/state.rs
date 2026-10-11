use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Starting,
    Asleep,
    Working,
    NeedsYou,
    Idle,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub claude_id: String,
    pub name: String,
    pub repo: String,
    pub workdir: String,
    #[serde(default)]
    pub worktree: bool,
    #[serde(default)]
    pub parent: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub exited: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default)]
    pub recent_repos: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<Session>,
}

fn version_one() -> u32 {
    1
}

impl Default for State {
    fn default() -> Self {
        Self { version: 1, recent_repos: vec![], sessions: vec![] }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reads state; a missing file is empty, a corrupt one is moved to `state.json.bak`.
pub fn load(path: &Path) -> State {
    let Ok(text) = fs::read_to_string(path) else {
        return State::default();
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(_) => {
            let _ = fs::rename(path, path.with_extension("json.bak"));
            State::default()
        }
    }
}

/// Atomic write: temp file, then rename.
pub fn save(path: &Path, state: &State) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(tmp, path)
}

/// First four words, lowercase `a-z0-9-`, at most 24 chars.
pub fn slug(prompt: &str) -> String {
    let joined = prompt
        .split_whitespace()
        .take(4)
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    let mut out = String::new();
    for c in joined.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.chars().take(24).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

pub fn unique_name(base: &str, taken: &[&str]) -> String {
    if !taken.contains(&base) {
        return base.into();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(&c.as_str()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sess(name: &str) -> Session {
        Session {
            id: format!("id-{name}"),
            claude_id: format!("c-{name}"),
            name: name.into(),
            repo: "/r".into(),
            workdir: "/r".into(),
            worktree: false,
            parent: None,
            created_at: 1,
            exited: false,
        }
    }

    #[test]
    fn slug_takes_first_four_words() {
        assert_eq!(slug("Fix the flaky auth test now"), "fix-the-flaky-auth");
    }

    #[test]
    fn slug_strips_punctuation_and_caps_at_24() {
        assert_eq!(slug("Render LaTeX: fractions!!! everywhere"), "render-latex-fractions-e");
    }

    #[test]
    fn slug_falls_back_to_session() {
        assert_eq!(slug(""), "session");
        assert_eq!(slug("   "), "session");
        assert_eq!(slug("🚀 🚀"), "session");
    }

    #[test]
    fn unique_name_suffixes() {
        assert_eq!(unique_name("b", &["a"]), "b");
        assert_eq!(unique_name("a", &["a", "a-2"]), "a-3");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/state.json");
        let st = State { version: 1, recent_repos: vec!["/r".into()], sessions: vec![sess("a")] };
        save(&p, &st).unwrap();
        assert_eq!(load(&p), st);
        assert!(!dir.path().join("nested/state.json.tmp").exists());
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("state.json")), State::default());
    }

    #[test]
    fn corrupt_file_moves_to_bak_and_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, "{nope").unwrap();
        assert_eq!(load(&p), State::default());
        assert_eq!(fs::read_to_string(dir.path().join("state.json.bak")).unwrap(), "{nope");
    }

    #[test]
    fn missing_fields_get_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, r#"{"sessions":[]}"#).unwrap();
        assert_eq!(load(&p).version, 1);
    }
}
