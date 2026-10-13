use std::path::{Path, PathBuf};

const DIR: &str = "Library/Application Support/petramux";
/// Where the app kept its data while it was called Roost.
const OLD_DIR: &str = "Library/Application Support/Roost";

/// The data folder under `home`: petramux's, or Roost's until the app has moved it.
fn dir_in(home: &Path) -> PathBuf {
    let new = home.join(DIR);
    let old = home.join(OLD_DIR);
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

/// Moves Roost's folder to petramux's (once, at app start), leaving a link at the old name:
/// sessions started before keep their old paths, and the helper keeps its socket.
pub fn migrate(home: &Path) -> std::io::Result<()> {
    let (old, new) = (home.join(OLD_DIR), home.join(DIR));
    let real_old = old.symlink_metadata().is_ok_and(|m| m.is_dir());
    if new.exists() || !real_old {
        return Ok(());
    }
    std::fs::rename(&old, &new)?;
    std::os::unix::fs::symlink(&new, &old)
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn default_dir() -> PathBuf {
        dir_in(Path::new(&std::env::var("HOME").unwrap_or_default()))
    }
    pub fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    pub fn sock(&self) -> PathBuf {
        self.dir.join("roost.sock")
    }
    /// The Claude theme sessions start in (`dark-ansi` / `light-ansi`), written by the app.
    pub fn claude_theme(&self) -> PathBuf {
        self.dir.join("claude-theme")
    }
    /// Where the terminal helper listens.
    pub fn pty_sock(&self) -> PathBuf {
        self.dir.join("pty.sock")
    }
    pub fn plugin(&self) -> PathBuf {
        self.dir.join("plugin")
    }
}

/// Socket for CLI calls: `$ROOST_SOCK` inside a session, else the default.
pub fn sock_from_env() -> PathBuf {
    std::env::var_os("ROOST_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| Paths::new(Paths::default_dir()).sock())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roost_folder_moves_to_petramux_and_leaves_a_link_behind() {
        let home = tempfile::tempdir().unwrap();
        let old = home.path().join(OLD_DIR);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("state.json"), "{}").unwrap();
        assert_eq!(dir_in(home.path()), old, "before the move, the old folder is the one in use");

        migrate(home.path()).unwrap();
        let new = home.path().join(DIR);
        assert_eq!(std::fs::read_to_string(new.join("state.json")).unwrap(), "{}");
        assert!(old.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(old.join("state.json")).unwrap(), "{}", "old paths still reach it");
        assert_eq!(dir_in(home.path()), new);

        migrate(home.path()).unwrap(); // a second start changes nothing
        assert!(old.symlink_metadata().unwrap().file_type().is_symlink());
        let fresh = tempfile::tempdir().unwrap();
        migrate(fresh.path()).unwrap();
        assert_eq!(dir_in(fresh.path()), fresh.path().join(DIR));
    }
}
