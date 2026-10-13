use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn default_dir() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Application Support/Roost")
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
