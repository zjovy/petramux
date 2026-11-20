//! Workspace chat: the sessions in a workspace share one chat that each of them, and you, can post
//! to. This module is the part without side effects: which chat a workspace has, who a message is
//! for, the log on disk, and the guard against agents talking in circles. The engine delivers.
use crate::state::name_key;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// A workspace's chat. The id outlives changes to the workspace: it follows the sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub name: String,
    /// Session ids, in the workspace's order.
    pub members: Vec<String>,
}

/// One line of a chat's log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Msg {
    /// When it was posted (ms since the epoch).
    pub ts: u64,
    /// The session's name, "user" for you, or "petramux" for a note from the app.
    pub from: String,
    pub text: String,
    /// The chat it belongs to (filled in when it's sent to the window, not stored).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chat: String,
}

pub const USER: &str = "user";
pub const APP: &str = "petramux";

/// The sessions in each workspace of a saved layout (`{spaces: [...]}`, or the single tree saved
/// before workspaces), leaving out diff and chat panes.
pub fn groups(layout: &Value) -> Vec<Vec<String>> {
    fn panes(n: &Value, out: &mut Vec<String>) {
        if let Some(p) = n["pane"].as_str() {
            if !p.contains(':') {
                out.push(p.into());
            }
        } else if n.is_object() {
            panes(&n["a"], out);
            panes(&n["b"], out);
        }
    }
    let trees: Vec<&Value> = match layout.get("spaces").and_then(Value::as_array) {
        Some(spaces) => spaces.iter().collect(),
        None if layout.is_object() => vec![layout],
        None => vec![],
    };
    trees
        .into_iter()
        .map(|t| {
            let mut out = vec![];
            panes(t, &mut out);
            out
        })
        .filter(|g| !g.is_empty())
        .collect()
}

/// Gives each workspace a chat: the old chat it shares the most sessions with (so a chat stays
/// with its workspace as sessions come and go, and the bigger side of a merge keeps its own), or
/// a new one named after its first session.
pub fn assign(old: &[Chat], groups: &[Vec<String>], name_of: impl Fn(&str) -> String) -> Vec<Chat> {
    let mut pairs: Vec<(usize, usize, usize)> = vec![]; // (shared, group, old chat)
    for (g, members) in groups.iter().enumerate() {
        for (c, chat) in old.iter().enumerate() {
            let shared = members.iter().filter(|m| chat.members.contains(m)).count();
            if shared > 0 {
                pairs.push((shared, g, c));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut chat_of: HashMap<usize, usize> = HashMap::new();
    let mut taken = HashSet::new();
    for (_, g, c) in pairs {
        if !chat_of.contains_key(&g) && taken.insert(c) {
            chat_of.insert(g, c);
        }
    }
    let mut names: Vec<String> = chat_of.values().map(|&c| old[c].name.clone()).collect();
    groups
        .iter()
        .enumerate()
        .map(|(g, members)| match chat_of.get(&g) {
            Some(&c) => Chat { members: members.clone(), ..old[c].clone() },
            None => {
                let taken: Vec<&str> = names.iter().map(String::as_str).collect();
                let name = crate::state::unique_display(&name_of(&members[0]), &taken);
                names.push(name.clone());
                Chat { id: uuid::Uuid::new_v4().to_string(), name, members: members.clone() }
            }
        })
        .collect()
}

/// Who a message is for: everyone else for `@all`, the sessions it names with `@name` (written
/// as the name, or with the spaces and punctuation left out: `@petra-bench`), else nobody. Never
/// its sender.
pub fn recipients(text: &str, members: &[(String, String)], sender: Option<&str>) -> Vec<String> {
    let others = members.iter().filter(|(id, _)| Some(id.as_str()) != sender);
    let lower = text.to_lowercase();
    let words: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter_map(|w| w.strip_prefix('@'))
        .map(name_key)
        .collect();
    if words.iter().any(|w| w == "all" || w == "everyone") {
        return others.map(|(id, _)| id.clone()).collect();
    }
    others
        .filter(|(_, name)| {
            let key = name_key(name);
            words.contains(&key) || (!key.is_empty() && named(&lower, &format!("@{}", name.to_lowercase())))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// `mention` (a name with spaces in it) appears in `text` as a whole: not the start of a longer word.
fn named(text: &str, mention: &str) -> bool {
    text.match_indices(mention).any(|(i, m)| !text[i + m.len()..].starts_with(|c: char| c.is_alphanumeric()))
}

/// How a message reads in a session's prompt.
pub fn delivered(chat: &str, from: &str, text: &str) -> String {
    format!("[#{} from {from}] {text}", chat_tag(chat))
}

/// A chat's name as a #tag: lowercase, spaces as dashes.
pub fn chat_tag(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase()
}

pub fn log_path(dir: &Path, chat: &str) -> PathBuf {
    dir.join("chats").join(format!("{chat}.jsonl"))
}

pub fn append(dir: &Path, chat: &str, msg: &Msg) -> std::io::Result<()> {
    let path = log_path(dir, chat);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(&Msg { chat: String::new(), ..msg.clone() })?;
    writeln!(f, "{line}")
}

/// The last `n` messages of a chat (a line that doesn't read is skipped).
pub fn tail(dir: &Path, chat: &str, n: usize) -> Vec<Msg> {
    let Ok(f) = std::fs::File::open(log_path(dir, chat)) else { return vec![] };
    let mut out: VecDeque<Msg> = VecDeque::new();
    for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        if let Ok(mut m) = serde_json::from_str::<Msg>(&line) {
            m.chat = chat.into();
            out.push_back(m);
            if out.len() > n {
                out.pop_front();
            }
        }
    }
    out.into()
}

/// Keeps agents from talking in circles: each session may post `PER_MINUTE` messages a minute,
/// and after `STREAK` messages between sessions with none from you, delivery stops until you post.
#[derive(Default)]
pub struct Guard {
    sent: HashMap<String, VecDeque<u64>>,
    streak: HashMap<String, u32>,
}

pub const PER_MINUTE: usize = 5;
pub const STREAK: u32 = 50;

#[derive(Debug)]
pub enum Verdict {
    Deliver,
    /// Logged and shown, but not delivered: the streak ran out (first time: say so in the chat).
    Hold { first: bool },
}

impl Guard {
    /// A session posts: too many this minute is refused outright.
    pub fn post(&mut self, chat: &str, sender: &str, now: u64) -> Result<Verdict, String> {
        let sent = self.sent.entry(sender.into()).or_default();
        while sent.front().is_some_and(|&t| now.saturating_sub(t) >= 60_000) {
            sent.pop_front();
        }
        if sent.len() >= PER_MINUTE {
            return Err(format!("slow down: at most {PER_MINUTE} messages a minute"));
        }
        sent.push_back(now);
        let streak = self.streak.entry(chat.into()).or_default();
        *streak += 1;
        Ok(if *streak <= STREAK { Verdict::Deliver } else { Verdict::Hold { first: *streak == STREAK + 1 } })
    }

    /// You posted: the sessions may talk again.
    pub fn user_posted(&mut self, chat: &str) {
        self.streak.remove(chat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat(id: &str, name: &str, members: &[&str]) -> Chat {
        Chat { id: id.into(), name: name.into(), members: members.iter().map(|m| m.to_string()).collect() }
    }
    fn g(members: &[&str]) -> Vec<String> {
        members.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn groups_are_the_sessions_of_each_workspace_without_diff_or_chat_panes() {
        let layout = json!({"spaces": [
            {"split": "row", "ratio": 0.5, "a": {"pane": "a"}, "b": {"split": "col", "ratio": 0.5, "a": {"pane": "b"}, "b": {"pane": "diff:b"}}},
            {"split": "row", "ratio": 0.7, "a": {"pane": "c"}, "b": {"pane": "chat:x"}},
            {"pane": "diff:z"}
        ], "popped": ["d"]});
        assert_eq!(groups(&layout), vec![g(&["a", "b"]), g(&["c"])]);
        assert_eq!(groups(&json!({"pane": "a"})), vec![g(&["a"])], "a layout from before workspaces");
        assert!(groups(&Value::Null).is_empty());
    }

    #[test]
    fn a_chat_follows_its_workspace() {
        let name = |id: &str| format!("name of {id}");
        let old = vec![chat("c1", "bench", &["a", "b", "c"]), chat("c2", "other", &["d"])];
        // a session joins, another leaves, workspaces trade order
        let next = assign(&old, &[g(&["d"]), g(&["a", "c", "e"])], name);
        assert_eq!(next, vec![chat("c2", "other", &["d"]), chat("c1", "bench", &["a", "c", "e"])]);
        // d's workspace merges into bench's: bench keeps its chat, d's chat ends
        let next = assign(&old, &[g(&["a", "b", "c", "d"])], name);
        assert_eq!(next, vec![chat("c1", "bench", &["a", "b", "c", "d"])]);
        // bench splits in two: the bigger side keeps it, the other gets a new chat
        let next = assign(&old, &[g(&["c"]), g(&["a", "b"])], name);
        assert_eq!((next[1].id.as_str(), next[0].name.as_str()), ("c1", "name of c"));
        assert_ne!(next[0].id, "c1");
        // a new workspace is named after its first session, never the same as another chat
        let next = assign(&[chat("c1", "name of x", &["a"])], &[g(&["a"]), g(&["x", "y"])], name);
        assert_eq!(next[1].name, "name of x 2");
    }

    #[test]
    fn a_message_goes_to_whoever_it_names() {
        let m: Vec<(String, String)> =
            [("1", "petra bench"), ("2", "profound"), ("3", "peec"), ("4", "semrush")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(recipients("@all done for today", &m, Some("2")), ["1", "3", "4"]);
        assert_eq!(recipients("@peec, @semrush: rerun please", &m, Some("1")), ["3", "4"]);
        assert_eq!(recipients("@petra-bench paused", &m, Some("2")), ["1"]);
        assert_eq!(recipients("@Petra Bench paused", &m, Some("2")), ["1"]);
        assert_eq!(recipients("@profound I am talking to myself", &m, Some("2")), Vec::<String>::new());
        assert_eq!(recipients("no one in particular", &m, Some("2")), Vec::<String>::new());
        assert_eq!(recipients("@peeca is someone else", &m, None), Vec::<String>::new());
        assert_eq!(recipients("@all from you", &m, None), ["1", "2", "3", "4"]);
    }

    #[test]
    fn a_delivered_message_says_where_and_who_from() {
        assert_eq!(delivered("Petra Bench", "peec", "@lead hi"), "[#petra-bench from peec] @lead hi");
    }

    #[test]
    fn the_log_keeps_every_message_and_reads_back_the_last_ones() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            append(dir.path(), "c1", &Msg { ts: i, from: "peec".into(), text: format!("line {i}\nsecond line"), chat: String::new() }).unwrap();
        }
        let last = tail(dir.path(), "c1", 2);
        assert_eq!(last.iter().map(|m| m.ts).collect::<Vec<_>>(), [3, 4]);
        assert_eq!((last[1].text.as_str(), last[1].chat.as_str()), ("line 4\nsecond line", "c1"));
        assert!(tail(dir.path(), "none", 5).is_empty());
    }

    #[test]
    fn the_guard_limits_each_sender_and_stops_long_agent_chains() {
        let mut gd = Guard::default();
        for i in 0..PER_MINUTE as u64 {
            assert!(matches!(gd.post("c", "peec", i * 1000), Ok(Verdict::Deliver)));
        }
        assert!(gd.post("c", "peec", 10_000).unwrap_err().contains("slow down"));
        assert!(gd.post("c", "peec", 61_000).is_ok(), "a minute later");

        let mut gd = Guard::default();
        let mut held = vec![];
        for i in 0..(STREAK + 3) {
            match gd.post("c", &format!("s{}", i % 7), u64::from(i) * 13_000).unwrap() {
                Verdict::Deliver => {}
                Verdict::Hold { first } => held.push(first),
            }
        }
        assert_eq!(held, [true, false, false]);
        gd.user_posted("c");
        assert!(matches!(gd.post("c", "s1", 999_999_999).unwrap(), Verdict::Deliver), "you posted: on again");
    }
}
