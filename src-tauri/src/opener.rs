//! ⌘-click on a link in a terminal: a web address opens in the browser, a file in the editor.
use std::path::{Path, PathBuf};

/// Programs `open` would run rather than show: these are only revealed in Finder.
const RUNNABLE: &[&str] = &["app", "command", "tool", "terminal", "workflow", "pkg", "dmg", "sh", "scpt"];

/// A path as a terminal shows it (absolute, `~/…`, or relative to `base`), if it exists.
pub fn resolve(base: &str, path: &str, home: &str) -> Option<PathBuf> {
    let p = match path.strip_prefix("~/") {
        Some(rest) => Path::new(home).join(rest),
        None => Path::new(base).join(path), // an absolute `path` replaces `base`
    };
    p.exists().then_some(p)
}

/// The program and arguments that open `target`: `open` for a web address, `editor -g file:line`
/// for a file (VS Code's `code`, when there is one), else `open` (or a reveal in Finder for
/// folders and anything `open` would run).
pub fn command(target: &str, base: &str, home: &str, line: Option<u32>, col: Option<u32>, editor: Option<&Path>) -> Option<(PathBuf, Vec<String>)> {
    let open = PathBuf::from("/usr/bin/open");
    if target.starts_with("https://") || target.starts_with("http://") {
        return Some((open, vec![target.into()]));
    }
    let path = match target.strip_prefix("file://") {
        Some(p) => decode(p.strip_prefix("localhost").unwrap_or(p)),
        None => target.to_string(),
    };
    let p = resolve(base, &path, home)?;
    let shown = p.display().to_string();
    if p.is_dir() {
        return Some((open, vec!["-R".into(), shown]));
    }
    if let Some(editor) = editor {
        let at = match (line, col) {
            (Some(l), Some(c)) => format!("{shown}:{l}:{c}"),
            (Some(l), None) => format!("{shown}:{l}"),
            _ => shown,
        };
        return Some((editor.to_path_buf(), vec!["-g".into(), at]));
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let runs = RUNNABLE.contains(&ext.as_str()) || executable(&p);
    Some((open, if runs { vec!["-R".into(), shown] } else { vec![shown] }))
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// `%20` and friends in a file URL.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|_| b[i] == b'%')
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(h) => {
                out.push(h);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(c: Option<(PathBuf, Vec<String>)>) -> Option<(String, Vec<String>)> {
        c.map(|(p, a)| (p.display().to_string(), a))
    }

    #[test]
    fn links_open_in_the_browser_or_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let b = base.display().to_string();
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/main.ts"), "").unwrap();
        std::fs::write(base.join("a b.png"), "").unwrap();
        std::fs::write(base.join("run.command"), "").unwrap();
        std::fs::write(base.join("tool"), "").unwrap();
        std::fs::set_permissions(base.join("tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let code = Path::new("/bin/code");
        let main = format!("{b}/src/main.ts");

        assert_eq!(args(command("https://x.dev/a?b=1", &b, "/h", None, None, Some(code))), Some(("/usr/bin/open".into(), vec!["https://x.dev/a?b=1".into()])));
        // relative to the session's folder, at a line and column
        assert_eq!(args(command("src/main.ts", &b, "/h", Some(12), Some(5), Some(code))), Some(("/bin/code".into(), vec!["-g".into(), format!("{main}:12:5")])));
        assert_eq!(args(command(&main, "/elsewhere", "/h", None, None, Some(code))).unwrap().1, vec!["-g".to_string(), main.clone()]);
        assert_eq!(args(command("~/src/main.ts", "/x", &b, Some(3), None, Some(code))).unwrap().1[1], format!("{main}:3"));
        assert_eq!(args(command(&format!("file://{b}/a%20b.png"), "/", "/h", None, None, None)), Some(("/usr/bin/open".into(), vec![format!("{b}/a b.png")])));
        // no editor: `open`, but never to run something
        assert_eq!(args(command("run.command", &b, "/h", None, None, None)).unwrap().1, vec!["-R".to_string(), format!("{b}/run.command")]);
        assert_eq!(args(command("tool", &b, "/h", None, None, None)).unwrap().1[0], "-R");
        assert_eq!(args(command("src", &b, "/h", None, None, Some(code))).unwrap().1, vec!["-R".to_string(), format!("{b}/src")]);
        // what isn't there isn't a link
        assert_eq!(command("nope.ts", &b, "/h", None, None, Some(code)), None);
        assert_eq!(command("javascript:alert(1)", &b, "/h", None, None, None), None);
    }
}
