//! Files dropped from Finder: the webview gets their bytes but never their path,
//! so they're saved here and the copy's path is pasted into the terminal.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Where dropped files go; macOS clears $TMPDIR of files nobody has opened in days.
pub fn root() -> PathBuf {
    std::env::temp_dir().join("petramux-drops")
}

/// Saves `bytes` under its own folder in `root`, keeping the file's name.
pub fn save(root: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = Path::new(name).file_name().unwrap_or("file".as_ref());
    let dir = root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_keeps_its_name_and_bytes_in_a_folder_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let a = save(root.path(), "shot.png", b"png").unwrap();
        let b = save(root.path(), "shot.png", b"other").unwrap();
        assert_eq!(a.file_name().unwrap(), "shot.png");
        assert_ne!(a, b);
        assert_eq!(fs::read(&a).unwrap(), b"png");
        assert_eq!(fs::read(&b).unwrap(), b"other");
    }

    #[test]
    fn a_name_cannot_climb_out_of_its_folder() {
        let root = tempfile::tempdir().unwrap();
        let p = save(root.path(), "../../escape.txt", b"x").unwrap();
        assert!(p.starts_with(root.path()));
        assert_eq!(p.file_name().unwrap(), "escape.txt");
        assert_eq!(save(root.path(), "..", b"x").unwrap().file_name().unwrap(), "file");
    }
}
