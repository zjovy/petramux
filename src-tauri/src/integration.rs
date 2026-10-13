use crate::paths::Paths;
use crate::pty::BLURB;
use crate::shellenv::{child_env, Env};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

/// `<support>/bin/claude`: runs the real claude with Roost's hooks, as this same process.
pub const SHIM: &str = r#"#!/bin/sh
# Roost: run the real claude with Roost's hooks, as this same process.
real=$ROOST_REAL_CLAUDE
if [ -z "$real" ] || [ ! -x "$real" ]; then
  # Roost found no claude at startup: run the next one on PATH, untracked.
  shim_dir=$(cd "$(dirname "$0")" && pwd)
  PATH=$(printf '%s\n' "$PATH" | tr ':' '\n' | grep -vxF "$shim_dir" | paste -sd: -)
  export PATH
  exec claude "$@"
fi
# A claude started by Claude itself (its Bash tool) must not touch this row.
if [ -n "$CLAUDECODE" ] || [ -z "$ROOST_BIN" ] || [ -z "$ROOST_PLUGIN" ]; then
  exec "$real" "$@"
fi
case "$1" in
  -v|--version|-h|--help) exec "$real" "$@" ;;
esac
if printf '%s' "$1" | grep -Eq '^[a-z][a-z-]*$'; then
  exec "$real" "$@"
fi
printf '{"pid":%d}' $$ | "$ROOST_BIN" hook ClaudeStart
# With a terminal theme on, Claude draws in its colors (the ANSI themes), light or dark to match.
theme=$(cat "${ROOST_PLUGIN%/plugin}/claude-theme" 2>/dev/null)
case "$theme" in
  dark-ansi|light-ansi) exec "$real" --plugin-dir "$ROOST_PLUGIN" --append-system-prompt "$ROOST_BLURB" --settings "{\"theme\":\"$theme\"}" "$@" ;;
esac
exec "$real" --plugin-dir "$ROOST_PLUGIN" --append-system-prompt "$ROOST_BLURB" "$@"
"#;

/// `<support>/zsh/.zshenv`: hands zsh back to the user's files, keeps the shim first,
/// and reports the first prompt so typed commands aren't eaten by startup scripts.
pub const ZSHENV: &str = r#"# Roost: hand zsh back to the user's own startup files, keep the claude shim first on PATH,
# and tell Roost once the first prompt is up.
if [[ -n $ROOST_ORIG_ZDOTDIR ]]; then ZDOTDIR=$ROOST_ORIG_ZDOTDIR; else unset ZDOTDIR; fi
[[ -f ${ZDOTDIR:-$HOME}/.zshenv ]] && source ${ZDOTDIR:-$HOME}/.zshenv
_roost_shim_first() { path=("$ROOST_SHIM_DIR" ${path:#$ROOST_SHIM_DIR}); }
_roost_ready() {
  precmd_functions=(${precmd_functions:#_roost_ready})
  "$ROOST_BIN" hook ShellReady </dev/null >/dev/null 2>&1
}
typeset -ga precmd_functions preexec_functions
precmd_functions+=(_roost_shim_first _roost_ready)
# preexec runs after every precmd hook (mise, direnv…) and before the command is looked up.
preexec_functions+=(_roost_shim_first)
"#;

/// `<support>/plugin/.claude-plugin/plugin.json`: hooks load with `--plugin-dir`, so a user's
/// own `--settings` keeps them (Claude reads only one `--settings`).
const PLUGIN_JSON: &str = r#"{"name":"petramux","version":"1.0.0","description":"Reports this Claude session's status to petramux."}"#;

/// The hooks of Roost's Claude Code plugin: one `roost hook <Event>` command per event.
pub fn hooks_json(roost_bin: &Path) -> Value {
    let bin = quote(&roost_bin.display().to_string());
    let entry = |matcher: Option<&str>, event: &str| {
        let mut e = json!({"hooks": [{"type": "command", "command": format!("{bin} hook {event}")}]});
        if let Some(m) = matcher {
            e["matcher"] = json!(m);
        }
        json!([e])
    };
    json!({"hooks": {
        "SessionStart": entry(None, "SessionStart"),
        "UserPromptSubmit": entry(None, "UserPromptSubmit"),
        "PostToolUse": entry(Some("*"), "PostToolUse"),
        "PostToolUseFailure": entry(Some("*"), "PostToolUseFailure"),
        "Notification": entry(Some("permission_prompt|elicitation_dialog"), "Notification"),
        // A question or a plan to approve; with permissions bypassed no Notification fires.
        "PreToolUse": entry(Some("AskUserQuestion|ExitPlanMode"), "PreToolUse"),
        "Stop": entry(None, "Stop"),
        "StopFailure": entry(None, "StopFailure"),
        "SessionEnd": entry(None, "SessionEnd"),
    }})
}

pub fn shim_dir(paths: &Paths) -> PathBuf {
    paths.dir.join("bin")
}

fn zdotdir(paths: &Paths) -> PathBuf {
    paths.dir.join("zsh")
}

/// Writes the hooks plugin, the shim and the zsh integration (on every launch).
pub fn install(paths: &Paths, roost_bin: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let plugin = paths.plugin();
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join("hooks"))?;
    std::fs::create_dir_all(shim_dir(paths))?;
    std::fs::create_dir_all(zdotdir(paths))?;
    std::fs::write(plugin.join(".claude-plugin/plugin.json"), PLUGIN_JSON)?;
    std::fs::write(plugin.join("hooks/hooks.json"), serde_json::to_vec_pretty(&hooks_json(roost_bin))?)?;
    let _ = std::fs::remove_file(paths.dir.join("hooks.json")); // written by Roost before the plugin
    let shim = shim_dir(paths).join("claude");
    std::fs::write(&shim, SHIM)?;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(zdotdir(paths).join(".zshenv"), ZSHENV)
}

/// One shell word, whatever it contains.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where a row's prompt waits to be read by its typed command.
pub fn prompt_file(paths: &Paths, id: &str) -> PathBuf {
    paths.dir.join("prompts").join(format!("{id}.txt"))
}

/// The prompt is read from a file, never typed: a terminal line holds ~1 KB, and control
/// characters typed into a shell are keystrokes. `--` keeps one-word and dash-leading
/// prompts a prompt (and tracked by the shim).
pub fn claude_cmd(prompt_file: &Path, worktree: Option<&str>) -> String {
    let arg = format!("\"$(cat {})\"", quote(&prompt_file.display().to_string()));
    match worktree {
        Some(name) => format!("claude -w {} -- {arg}", quote(name)),
        None => format!("claude -- {arg}"),
    }
}

pub fn fork_cmd(claude_id: &str) -> String {
    format!("claude --resume {} --fork-session", quote(claude_id))
}

/// Brings back the Claude a row had: its saved conversation, or a fresh Claude if none was saved.
pub fn reopen_cmd(workdir: &str, saved_id: Option<&str>) -> String {
    match saved_id {
        Some(id) => format!("cd {} && claude --resume {}", quote(workdir), quote(id)),
        None => format!("cd {} && claude", quote(workdir)),
    }
}

/// Env for a session's shell: v1's child env plus the shim and Roost's variables.
pub fn session_env(base: &Env, paths: &Paths, roost_bin: &Path, claude: &Path, shell: &Path, id: &str) -> Env {
    let mut env = child_env(base, id, &paths.sock());
    let shim = shim_dir(paths).display().to_string();
    let path = env.get("PATH").map(|p| format!("{shim}:{p}")).unwrap_or(shim.clone());
    env.insert("PATH".into(), path);
    env.insert("ROOST_SHIM_DIR".into(), shim);
    env.insert("ROOST_BIN".into(), roost_bin.display().to_string());
    env.insert("ROOST_REAL_CLAUDE".into(), claude.display().to_string());
    env.insert("ROOST_PLUGIN".into(), paths.plugin().display().to_string());
    env.insert("ROOST_BLURB".into(), BLURB.into());
    if shell.file_name().is_some_and(|n| n == "zsh") {
        if let Some(orig) = env.get("ZDOTDIR").cloned() {
            env.insert("ROOST_ORIG_ZDOTDIR".into(), orig);
        }
        env.insert("ZDOTDIR".into(), zdotdir(paths).display().to_string());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn hooks_cover_nine_events() {
        let v = hooks_json(Path::new("/Apps/It's Roost/roost"));
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 9);
        assert_eq!(hooks["PostToolUseFailure"][0]["matcher"], "*");
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "AskUserQuestion|ExitPlanMode");
        assert_eq!(hooks["StopFailure"][0]["hooks"][0]["command"], r"'/Apps/It'\''s Roost/roost' hook StopFailure");
        assert_eq!(hooks["SessionEnd"][0]["hooks"][0]["command"], r"'/Apps/It'\''s Roost/roost' hook SessionEnd");
        assert_eq!(hooks["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
    }

    #[test]
    fn quoting_makes_shell_syntax_inert() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        let nasty = "'; rm -rf ~; ' $(whoami) `id` \n next";
        let out = Command::new("/bin/sh").args(["-c", &format!("printf %s {}", quote(nasty))]).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), nasty);
    }

    #[test]
    fn typed_commands() {
        assert_eq!(claude_cmd(Path::new("/s/p/a b.txt"), None), r#"claude -- "$(cat '/s/p/a b.txt')""#);
        assert_eq!(claude_cmd(Path::new("/s/p/a.txt"), Some("wt")), r#"claude -w 'wt' -- "$(cat '/s/p/a.txt')""#);
        assert_eq!(reopen_cmd("/r/a b", Some("c1")), "cd '/r/a b' && claude --resume 'c1'");
        assert_eq!(reopen_cmd("/r/a b", None), "cd '/r/a b' && claude");
        assert_eq!(fork_cmd("c1"), "claude --resume 'c1' --fork-session");
    }

    #[test]
    fn install_writes_executable_shim_and_zshenv() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::write(paths.dir.join("hooks.json"), "{}").unwrap(); // left by a pre-plugin Roost
        install(&paths, Path::new("/x/roost")).unwrap();
        let shim = paths.dir.join("bin/claude");
        assert_eq!(std::fs::metadata(&shim).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_to_string(shim).unwrap(), SHIM);
        assert_eq!(std::fs::read_to_string(paths.dir.join("zsh/.zshenv")).unwrap(), ZSHENV);
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.plugin().join(".claude-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "petramux");
        let hooks = std::fs::read_to_string(paths.plugin().join("hooks/hooks.json")).unwrap();
        assert!(hooks.contains("'/x/roost' hook SessionEnd"));
        assert!(!paths.dir.join("hooks.json").exists());
    }

    /// Runs the real shim with a fake claude (prints args) and a fake roost (records the hook).
    fn run_shim_with(args: &[&str], extra: &[(&str, String)]) -> (String, String) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        install(&paths, Path::new("/unused")).unwrap();
        let fake_claude = dir.path().join("real-claude");
        exe(&fake_claude, "#!/bin/sh
printf '%s|' \"$@\"
");
        let fake_roost = dir.path().join("fake-roost");
        let log = dir.path().join("hook.log");
        exe(&fake_roost, &format!("#!/bin/sh
echo \"$1 $2 $(cat)\" >> '{}'
", log.display()));
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        exe(&other.join("claude"), "#!/bin/sh
printf 'fallback|'; printf '%s|' \"$@\"
");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(paths.dir.join("bin/claude"))
            .args(args)
            .env_clear()
            .env("PATH", format!("{}:{}:/usr/bin:/bin", shim_dir(&paths).display(), other.display()))
            .env("ROOST_REAL_CLAUDE", &fake_claude)
            .env("ROOST_BIN", &fake_roost)
            .env("ROOST_PLUGIN", "/plug")
            .env("ROOST_BLURB", "B");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = crate::shellenv::run_with_timeout(cmd, std::time::Duration::from_secs(5)).expect("shim hung");
        (String::from_utf8(out).unwrap(), std::fs::read_to_string(log).unwrap_or_default())
    }

    fn run_shim(args: &[&str]) -> (String, String) {
        run_shim_with(args, &[])
    }

    #[test]
    fn shim_leaves_a_claude_started_by_claude_alone() {
        assert_eq!(run_shim_with(&["--", "x"], &[("CLAUDECODE", "1".into())]), ("--|x|".into(), String::new()));
    }

    #[test]
    fn shim_without_a_known_claude_runs_the_next_one_on_path_untracked() {
        assert_eq!(
            run_shim_with(&["--", "x"], &[("ROOST_REAL_CLAUDE", String::new())]),
            ("fallback|--|x|".into(), String::new())
        );
    }

    #[test]
    fn shim_adds_hooks_and_reports_its_pid() {
        let (args, hook) = run_shim(&["--", "fix it"]);
        assert_eq!(args, "--plugin-dir|/plug|--append-system-prompt|B|--|fix it|");
        assert!(hook.starts_with("hook ClaudeStart {\"pid\":"), "{hook}");
    }

    #[test]
    fn shim_passes_subcommands_and_version_through() {
        assert_eq!(run_shim(&["mcp", "list"]), ("mcp|list|".into(), String::new()));
        assert_eq!(run_shim(&["--version"]), ("--version|".into(), String::new()));
    }

    #[test]
    fn zsh_keeps_the_shim_first_after_zshrc_reorders_path() {
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("support"));
        install(&paths, Path::new("/usr/bin/true")).unwrap();
        let home = dir.path().join("home");
        let other = dir.path().join("other");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        exe(&other.join("claude"), "#!/bin/sh\n");
        std::fs::write(home.join(".zshrc"), format!("path=('{}' $path)\n", other.display())).unwrap();
        let env = session_env(
            &[("PATH".to_string(), "/usr/bin:/bin".to_string()), ("HOME".to_string(), home.display().to_string())]
                .into_iter()
                .collect(),
            &paths,
            Path::new("/usr/bin/true"),
            Path::new("/real/claude"),
            Path::new("/bin/zsh"),
            "s1",
        );
        let out = Command::new("/bin/zsh")
            .args(["-l", "-i", "-c", "for f in $precmd_functions; do $f; done; command -v claude"])
            .env_clear()
            .envs(&env)
            .output()
            .unwrap();
        let got = String::from_utf8(out.stdout).unwrap();
        assert_eq!(got.trim(), shim_dir(&paths).join("claude").display().to_string(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn zsh_keeps_the_shim_first_after_later_precmd_hooks() {
        // Like mise or direnv: a precmd hook added by .zshrc, running after Roost's.
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("support"));
        install(&paths, Path::new("/usr/bin/true")).unwrap();
        let home = dir.path().join("home");
        let other = dir.path().join("other");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        exe(&other.join("claude"), "#!/bin/sh\n");
        std::fs::write(
            home.join(".zshrc"),
            format!("_late() {{ path=('{}' $path) }}\nprecmd_functions+=(_late)\n", other.display()),
        )
        .unwrap();
        let env = session_env(
            &[("PATH".to_string(), "/usr/bin:/bin".to_string()), ("HOME".to_string(), home.display().to_string())]
                .into_iter()
                .collect(),
            &paths,
            Path::new("/usr/bin/true"),
            Path::new("/real/claude"),
            Path::new("/bin/zsh"),
            "s1",
        );
        let script = "for f in $precmd_functions; do $f; done; for f in $preexec_functions; do $f; done; command -v claude";
        let out = Command::new("/bin/zsh").args(["-l", "-i", "-c", script]).env_clear().envs(&env).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), shim_dir(&paths).join("claude").display().to_string());
    }

    #[test]
    fn session_env_sets_roost_vars_and_zdotdir_for_zsh_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mut base = Env::new();
        base.insert("PATH".into(), "/bin".into());
        base.insert("ZDOTDIR".into(), "/u/zdot".into());
        let z = session_env(&base, &paths, Path::new("/rb"), Path::new("/rc"), Path::new("/bin/zsh"), "s1");
        assert!(z["PATH"].starts_with(&format!("{}:", shim_dir(&paths).display())));
        assert_eq!(z["ROOST_BIN"], "/rb");
        assert_eq!(z["ROOST_REAL_CLAUDE"], "/rc");
        assert_eq!(z["ROOST_PLUGIN"], paths.plugin().display().to_string());
        assert!(!z.contains_key("ROOST_HOOKS"));
        assert_eq!(z["ROOST_BLURB"], BLURB);
        assert_eq!(z["ROOST_SESSION"], "s1");
        assert_eq!(z["ZDOTDIR"], paths.dir.join("zsh").display().to_string());
        assert_eq!(z["ROOST_ORIG_ZDOTDIR"], "/u/zdot");
        let b = session_env(&base, &paths, Path::new("/rb"), Path::new("/rc"), Path::new("/bin/bash"), "s1");
        assert_eq!(b["ZDOTDIR"], "/u/zdot");
        assert!(!b.contains_key("ROOST_ORIG_ZDOTDIR"));
    }
}
