//! Looks, the way cmux gets them: from the user's Ghostty config and Ghostty's theme files.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Colors by their Ghostty key: `background`, `foreground`, `cursor-color`, `cursor-text`,
/// `selection-background`, `selection-foreground`, and `palette-0` … `palette-255`.
pub type Colors = BTreeMap<String, String>;

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    /// The config's `theme`: a name, a path, or `light:A,dark:B`.
    pub theme: Option<String>,
    /// `font-family` lines in order: the first is the font, the rest are fallbacks.
    pub font_family: Vec<String>,
    pub font_size: Option<f64>,
    /// Colors set in the config itself; they win over the theme's.
    pub colors: Colors,
}

/// Ghostty's config files, read in this order (later lines win).
pub fn config_files(home: &Path) -> Vec<PathBuf> {
    let xdg = home.join(".config/ghostty");
    let mac = home.join("Library/Application Support/com.mitchellh.ghostty");
    [xdg.join("config"), xdg.join("config.ghostty"), mac.join("config"), mac.join("config.ghostty")].into()
}

/// Where theme files live: the user's own first, then the ones bundled with the app
/// (Ghostty's set, from iTerm2-Color-Schemes; see its LICENSE there).
pub fn theme_dirs(home: &Path, bundled: &Path) -> Vec<PathBuf> {
    vec![home.join(".config/ghostty/themes"), bundled.to_path_buf()]
}

/// Sets the Claude theme petramux's sessions use: `dark-ansi` / `light-ansi` to match the terminal,
/// or none. Claude picks a fixed theme once, at launch, but rereads its custom themes (`themes`, in
/// Claude's config folder) when they change: so the mode goes into the `petramux` custom theme and
/// sessions start in that, and running ones follow when the terminal theme flips light or dark.
pub fn set_claude_theme(marker: &Path, themes: &Path, mode: Option<&str>) -> Result<(), String> {
    let Some(mode) = mode else {
        return match fs::remove_file(marker) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    };
    if !matches!(mode, "dark-ansi" | "light-ansi") {
        return Err(format!("not a Claude theme: {mode}"));
    }
    let custom = serde_json::json!({ "name": "petramux (matches the terminal)", "base": mode, "overrides": {} });
    let live = fs::create_dir_all(themes).and_then(|_| fs::write(themes.join("petramux.json"), custom.to_string()));
    fs::write(marker, if live.is_ok() { "custom:petramux" } else { mode }).map_err(|e| e.to_string())
}

pub fn appearance(home: &Path) -> Appearance {
    let mut a = Appearance::default();
    for file in config_files(home) {
        let Ok(text) = fs::read_to_string(file) else { continue };
        for (key, value) in parse(&text) {
            match key.as_str() {
                "theme" => a.theme = Some(value).filter(|v| !v.is_empty()),
                "font-family" if value.is_empty() => a.font_family.clear(),
                "font-family" => a.font_family.push(value),
                "font-size" => a.font_size = value.parse().ok().or(a.font_size),
                _ => {
                    if let Some((k, v)) = color(&key, &value) {
                        a.colors.insert(k, v);
                    }
                }
            }
        }
    }
    a
}

/// Every theme name, once, sorted.
pub fn list(dirs: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = dirs
        .iter()
        .flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "LICENSE")
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// A theme's colors, by name (the first dir that has it) or by path.
pub fn load(dirs: &[PathBuf], name: &str) -> Option<Colors> {
    let file = if name.contains('/') {
        PathBuf::from(name)
    } else {
        dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())?
    };
    let text = fs::read_to_string(file).ok()?;
    Some(parse(&text).into_iter().filter_map(|(k, v)| color(&k, &v)).collect())
}

/// `key = value` lines; `#` comments, blank lines and surrounding quotes dropped.
fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), unquote(v.trim()).to_string()))
        .collect()
}

fn unquote(v: &str) -> &str {
    v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v)
}

/// A color line in Ghostty's form; `palette = 3=#f9e2af` becomes `palette-3`.
fn color(key: &str, value: &str) -> Option<(String, String)> {
    const KEYS: [&str; 6] =
        ["background", "foreground", "cursor-color", "cursor-text", "selection-background", "selection-foreground"];
    if key == "palette" {
        let (n, c) = value.split_once('=')?;
        let n: u8 = n.trim().parse().ok()?;
        return Some((format!("palette-{n}"), hex(c.trim())?));
    }
    KEYS.contains(&key).then(|| hex(value)).flatten().map(|c| (key.to_string(), c))
}

/// `#rrggbb` (Ghostty also takes it without the `#`).
fn hex(v: &str) -> Option<String> {
    let h = v.strip_prefix('#').unwrap_or(v);
    (h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", h.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn claude_follows_the_terminal_through_its_custom_theme() {
        let dir = tempfile::tempdir().unwrap();
        let (marker, themes) = (dir.path().join("claude-theme"), dir.path().join(".claude/themes"));
        set_claude_theme(&marker, &themes, Some("light-ansi")).unwrap();
        let t: serde_json::Value = serde_json::from_str(&fs::read_to_string(themes.join("petramux.json")).unwrap()).unwrap();
        assert_eq!(t["base"], "light-ansi");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "custom:petramux");

        set_claude_theme(&marker, &themes, Some("dark-ansi")).unwrap();
        assert!(fs::read_to_string(themes.join("petramux.json")).unwrap().contains("\"dark-ansi\""));

        // no theme: new sessions start as the user set Claude up; running ones keep the last colors
        set_claude_theme(&marker, &themes, None).unwrap();
        assert!(!marker.exists() && themes.join("petramux.json").exists());
        assert!(set_claude_theme(&marker, &themes, Some("dark; rm -rf ~")).is_err());

        // Claude's themes folder can't be written: sessions start in the fixed theme, as before
        let blocked = dir.path().join("file");
        fs::write(&blocked, "").unwrap();
        set_claude_theme(&marker, &blocked.join("themes"), Some("light-ansi")).unwrap();
        assert_eq!(fs::read_to_string(&marker).unwrap(), "light-ansi");
    }

    #[test]
    fn theme_files_read_like_ghostty() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("Mocha"), "# a comment\n\npalette = 3=#F9E2AF\npalette=15=bac2de\nbackground = #1e1e2e\ncursor-color = \"#f5e0dc\"\nfont-size = 99\npalette = 300=#000000\nforeground = nope\n");
        let c = load(&[dir.path().to_path_buf()], "Mocha").unwrap();
        assert_eq!(c.get("palette-3").unwrap(), "#f9e2af");
        assert_eq!(c.get("palette-15").unwrap(), "#bac2de");
        assert_eq!(c.get("background").unwrap(), "#1e1e2e");
        assert_eq!(c.get("cursor-color").unwrap(), "#f5e0dc");
        assert_eq!(c.len(), 4); // no font-size, no palette 300, no bad hex
    }

    #[test]
    fn config_files_stack_and_later_lines_win() {
        let home = tempfile::tempdir().unwrap();
        write(&home.path().join(".config/ghostty/config"), "theme = Dracula\nfont-family = \"JetBrains Mono\"\nfont-size = 13\nbackground = 101010\n");
        write(
            &home.path().join("Library/Application Support/com.mitchellh.ghostty/config.ghostty"),
            "theme = light:Latte,dark:Mocha\nfont-family = Symbols Nerd Font\nforeground = #eeeeee\n",
        );
        let a = appearance(home.path());
        assert_eq!(a.theme.as_deref(), Some("light:Latte,dark:Mocha"));
        assert_eq!(a.font_family, ["JetBrains Mono", "Symbols Nerd Font"]);
        assert_eq!(a.font_size, Some(13.0));
        assert_eq!(a.colors.get("background").unwrap(), "#101010");
        assert_eq!(a.colors.get("foreground").unwrap(), "#eeeeee");
    }

    #[test]
    fn an_empty_font_family_line_resets_the_list() {
        let home = tempfile::tempdir().unwrap();
        write(&home.path().join(".config/ghostty/config"), "font-family = A\nfont-family = \"\"\nfont-family = B\n");
        assert_eq!(appearance(home.path()).font_family, ["B"]);
    }

    #[test]
    fn no_config_means_no_opinions() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(appearance(home.path()), Appearance::default());
    }

    #[test]
    fn the_users_themes_come_first_and_names_list_once() {
        let user = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(&user.path().join("Dracula"), "background = #000001\n");
        write(&bundled.path().join("Dracula"), "background = #000002\n");
        write(&bundled.path().join("ayu"), "background = #000003\n");
        write(&bundled.path().join(".DS_Store"), "");
        write(&bundled.path().join("LICENSE"), "MIT License");
        let dirs = [user.path().to_path_buf(), bundled.path().to_path_buf(), user.path().join("missing")];
        assert_eq!(list(&dirs), ["ayu", "Dracula"]);
        assert_eq!(load(&dirs, "Dracula").unwrap().get("background").unwrap(), "#000001");
        assert_eq!(load(&dirs, &bundled.path().join("ayu").display().to_string()).unwrap().len(), 1);
        assert!(load(&dirs, "Nope").is_none());
    }
}
