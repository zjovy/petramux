use crate::engine::{Attention, Core, Diff, NewSession, Place, Setup, Sink, View};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use crate::past::Past;
use crate::paths::Paths;
use crate::state::{Settings, Status};
use crate::outputs::{Out, Outputs};
use crate::{drops, opener, shellenv, sock, theme, update};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

impl Out for Channel<String> {
    fn send(&self, text: String) -> bool {
        Channel::send(self, text).is_ok()
    }
}

struct UiSink {
    app: AppHandle,
    /// Each terminal's output, to every window showing it.
    outputs: Mutex<Outputs<Channel<String>>>,
    visible: Mutex<HashSet<String>>,
}

/// The label of a session's own window.
fn own_window(id: &str) -> String {
    format!("s-{id}")
}

impl UiSink {
    fn attach(&self, id: String, window: &str, ch: Channel<String>) {
        self.outputs.lock().unwrap().attach(&id, window, ch);
    }

    fn detach(&self, id: &str) {
        self.outputs.lock().unwrap().detach(id);
    }

    fn badge(&self, views: &[View]) {
        let n = views.iter().filter(|v| v.status == Status::NeedsYou).count() as i64;
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.set_badge_count(if n > 0 { Some(n) } else { None });
        }
    }
}

impl Sink for UiSink {
    fn sessions_changed(&self, views: Vec<View>) {
        self.badge(&views);
        let _ = self.app.emit("sessions_changed", views);
    }

    fn session_changed(&self, view: View) {
        let _ = self.app.emit("session_changed", &view);
        if let Some(core) = self.app.try_state::<Arc<Core>>() {
            self.badge(&core.views());
        }
    }

    fn output(&self, id: &str, text: String) {
        self.outputs.lock().unwrap().output(id, text);
    }

    fn chat_message(&self, msg: &crate::chat::Msg) {
        let _ = self.app.emit("chat_message", msg);
    }

    fn chats_changed(&self, chats: Vec<crate::chat::Chat>) {
        let _ = self.app.emit("chats_changed", chats);
    }

    fn attention(&self, view: &View, kind: Attention) {
        if let Some(core) = self.app.try_state::<Arc<Core>>() {
            let s = core.settings();
            let wanted = match kind {
                Attention::NeedsYou => s.notify_needs_you,
                Attention::Done => s.notify_done,
            };
            if !wanted {
                return;
            }
        }
        let focused = self
            .app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
        let shown = self.visible.lock().unwrap().contains(&view.id);
        // or in a window of its own, in front
        let own = self.app.get_webview_window(&own_window(&view.id)).and_then(|w| w.is_focused().ok()).unwrap_or(false);
        if (focused && shown) || own {
            return;
        }
        let body = view.note.clone().unwrap_or_else(|| match kind {
            Attention::NeedsYou => "Needs you".into(),
            Attention::Done => "Done".into(),
        });
        let _ = self.app.notification().builder().title(&view.name).body(body).show();
    }
}

type CoreState<'a> = State<'a, Arc<Core>>;
type SinkState<'a> = State<'a, Arc<UiSink>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boot {
    sessions: Vec<View>,
    recent_repos: Vec<String>,
    claude_found: bool,
    layout: Option<Value>,
    focus: Option<String>,
    shell: String,
    settings: Settings,
    restored: Vec<String>,
}

#[tauri::command]
fn boot(core: CoreState) -> Boot {
    let (layout, focus) = core.layout();
    Boot {
        sessions: core.views(),
        recent_repos: core.recent_repos(),
        claude_found: core.claude_found(),
        layout,
        focus,
        shell: core.shell_name(),
        settings: core.settings(),
        restored: core.restored(),
    }
}

#[tauri::command]
fn set_settings(settings: Settings, core: CoreState) {
    core.set_settings(settings);
}

#[tauri::command]
fn attach(id: String, output: Channel<String>, window: tauri::Window, sink: SinkState) {
    sink.attach(id, window.label(), output);
}

/// A session in a window of its own: where it was dropped, else where that window last was.
#[tauri::command]
fn open_window(id: String, name: String, x: Option<f64>, y: Option<f64>, app: AppHandle) -> Result<(), String> {
    let label = own_window(&id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    // the page reads which session it shows from here (see main.ts)
    let script = format!("window.__PETRAMUX_SESSION__ = {};", serde_json::to_string(&id).map_err(|e| e.to_string())?);
    let w = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html".into()))
        .title(&name)
        .inner_size(960.0, 640.0)
        .min_inner_size(420.0, 300.0)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16.0, 18.0)) // centered on the 36px top bar
        .background_color(tauri::window::Color(15, 15, 17, 255))
        .disable_drag_drop_handler()
        .initialization_script(&script)
        .build()
        .map_err(|e| e.to_string())?;
    if let (Some(x), Some(y)) = (x, y) {
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
    Ok(())
}

#[tauri::command]
fn focus_window(id: String, app: AppHandle) {
    if let Some(w) = app.get_webview_window(&own_window(&id)) {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Closes a session's own window (the session goes back to the main window, still running).
#[tauri::command]
fn close_window(id: String, app: AppHandle) {
    if let Some(w) = app.get_webview_window(&own_window(&id)) {
        let _ = w.close();
    }
}

#[tauri::command]
fn select_session(id: String, core: CoreState) -> Result<View, String> {
    core.select(&id)
}

#[tauri::command]
fn new_session(prompt: String, repo: String, worktree: bool, parent: Option<String>, core: CoreState) -> Result<View, String> {
    core.new_session(NewSession { prompt, repo, worktree, name: None, parent })
}

// async: git can take a moment on a big repo; never on the main thread.
#[tauri::command]
async fn diff(id: String, scope: String, core: CoreState<'_>) -> Result<Diff, String> {
    core.diff(&id, &scope)
}

// async: git can take a moment; never on the main thread.
#[tauri::command]
async fn place(id: String, core: CoreState<'_>) -> Result<Place, String> {
    core.place(&id)
}

// async: a session that stops reading may block this write; never on the main thread.
#[tauri::command]
async fn write(id: String, data: String, core: CoreState<'_>) -> Result<(), String> {
    core.write(&id, data.as_bytes())
}

#[tauri::command]
fn resize(id: String, cols: u16, rows: u16, core: CoreState) {
    core.resize(&id, cols, rows);
}

#[tauri::command]
fn reopen_closed(core: CoreState) -> Result<View, String> {
    core.reopen_closed()
}

#[tauri::command]
fn restart_claude(id: String, core: CoreState) -> Result<(), String> {
    core.restart_claude(&id)
}

#[tauri::command]
fn fork(id: String, core: CoreState) -> Result<View, String> {
    core.fork(&id)
}

/// ⌘-click on a link: a web address in the browser, a file in the editor (see `opener`).
#[tauri::command]
async fn open_link(target: String, base: String, line: Option<u32>, col: Option<u32>, core: CoreState<'_>) -> Result<(), String> {
    let editor = core.find_bin("code");
    let home = home().display().to_string();
    let (program, args) = opener::command(&target, &base, &home, line, col, editor.as_deref()).ok_or("nothing there to open")?;
    let mut child = std::process::Command::new(program).args(args).spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || child.wait()); // reaped, not left a zombie
    Ok(())
}

/// Which of the path-like words in a terminal line are files or folders (only those become links).
#[tauri::command]
async fn existing_paths(base: String, paths: Vec<String>) -> Vec<bool> {
    let home = home().display().to_string();
    paths.iter().map(|p| opener::resolve(&base, p, &home).is_some()).collect()
}

/// A newer release is in place, or a newer build of this repo is ready to install (see `update`).
#[tauri::command]
async fn update_ready() -> bool {
    let Some(installed) = std::env::current_exe().ok().and_then(|e| update::app_of(&e)) else { return false };
    update::release_installed() || update::newer(&update::built(), &installed, std::time::SystemTime::now())
}

#[tauri::command]
fn restart(app: AppHandle) -> Result<(), String> {
    restart_app(&app)
}

/// Quits keeping every session (whatever Settings says about quitting), installs a newer build if
/// there is one, and opens petramux again.
fn restart_app(app: &AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed = update::app_of(&exe).ok_or("petramux isn't running from an app")?;
    let build = update::built();
    let from = update::newer(&build, &installed, std::time::SystemTime::now()).then_some(build.as_path());
    update::restart(std::process::id(), from, &installed, Path::new("/usr/bin/open")).map_err(|e| e.to_string())?;
    remember_window(app);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        core.detach();
    }
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn chats(core: CoreState) -> Vec<crate::chat::Chat> {
    core.chats()
}

#[tauri::command]
fn chat_log(chat: String, core: CoreState) -> Vec<crate::chat::Msg> {
    core.chat_log(&chat, 500)
}

#[tauri::command]
async fn chat_say(chat: String, text: String, core: CoreState<'_>) -> Result<(), String> {
    core.say_user(&chat, &text)
}

#[tauri::command]
fn rename_chat(chat: String, name: String, core: CoreState) -> Result<(), String> {
    core.rename_chat(&chat, &name)
}

#[tauri::command]
fn rename(id: String, name: String, core: CoreState) -> Result<View, String> {
    core.rename(&id, &name)
}

#[tauri::command]
fn close(id: String, core: CoreState, sink: SinkState) {
    core.close(&id);
    sink.detach(&id);
}

/// Pane ids in a saved layout tree.
fn pane_ids(node: Option<&Value>) -> HashSet<String> {
    fn walk(n: &Value, out: &mut HashSet<String>) {
        if let Some(id) = n["pane"].as_str() {
            out.insert(id.to_string());
        }
        for k in ["a", "b"] {
            if let Some(child) = n.get(k) {
                walk(child, out);
            }
        }
    }
    let mut out = HashSet::new();
    if let Some(n) = node {
        walk(n, &mut out);
    }
    out
}

/// The panes on screen: the workspace holding the focused pane (or, saved before workspaces, the one tree).
fn visible_panes(layout: Option<&Value>, focus: Option<&str>) -> HashSet<String> {
    let Some(spaces) = layout.and_then(|l| l["spaces"].as_array()) else { return pane_ids(layout) };
    let focus = focus.unwrap_or_default();
    spaces.iter().map(|t| pane_ids(Some(t))).find(|ids| ids.contains(focus)).unwrap_or_default()
}

#[tauri::command]
fn save_layout(layout: Option<Value>, focus: Option<String>, core: CoreState, sink: SinkState) {
    *sink.visible.lock().unwrap() = visible_panes(layout.as_ref(), focus.as_deref());
    core.save_layout(layout, focus);
}

#[tauri::command]
async fn list_past(repo: String, core: CoreState<'_>) -> Result<Vec<Past>, String> {
    Ok(core.list_past(&repo))
}

#[tauri::command]
fn resume_past(repo: String, claude_id: String, title: String, workdir: String, core: CoreState) -> Result<View, String> {
    core.resume_past(&repo, Past { claude_id, title, workdir, modified_ms: 0 })
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// The user's Ghostty config: theme, fonts and colors, the way cmux reads them.
#[tauri::command]
fn appearance() -> theme::Appearance {
    theme::appearance(&home())
}

// async: listing ~460 files is quick, but never on the main thread.
#[tauri::command]
async fn themes(app: AppHandle) -> Vec<String> {
    theme::list(&theme_dirs(&app))
}

#[tauri::command]
fn theme(name: String, app: AppHandle) -> Option<theme::Colors> {
    theme::load(&theme_dirs(&app), &name)
}

fn theme_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let bundled = app.path().resource_dir().map(|d| d.join("themes")).unwrap_or_default();
    theme::theme_dirs(&home(), &bundled)
}

/// The Claude theme sessions use: the ANSI one matching the terminal theme, or none.
#[tauri::command]
fn set_claude_theme(mode: Option<String>) -> Result<(), String> {
    let marker = Paths::new(Paths::default_dir()).claude_theme();
    theme::set_claude_theme(&marker, &home().join(".claude/themes"), mode.as_deref())
}

/// A file dropped on a terminal: its bytes come raw, its (already safe) name in a header.
#[tauri::command]
async fn save_drop(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the file's bytes".into());
    };
    let name = request.headers().get("name").and_then(|v| v.to_str().ok()).unwrap_or("file");
    drops::save(&drops::root(), name, bytes)
        .map(|p| p.display().to_string())
        .map_err(|e| format!("could not save the dropped file: {e}"))
}

// async: the blocking dialog must not run on the main thread.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().map(|p| p.to_string())
}

#[tauri::command]
async fn ask(app: AppHandle, message: String, ok: String) -> bool {
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(ok, "Cancel".into()))
        .blocking_show()
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItem::with_id(app, "quit", "Quit petramux", true, Some("CmdOrCtrl+Q"))?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let shortcuts = MenuItem::with_id(app, "shortcuts", "Keyboard Shortcuts", true, Some("CmdOrCtrl+/"))?;
    // No shortcut: in Apple's apps ⌥⌘Q means "quit and keep windows", the opposite of this.
    let stop_all = MenuItem::with_id(app, "stop_all", "Stop All Sessions and Quit", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "Restart petramux (Sessions Keep Running)", true, None::<&str>)?;
    let roost = Submenu::with_items(app, "petramux", true, &[
        &PredefinedMenuItem::about(app, None, None)?,
        &PredefinedMenuItem::separator(app)?,
        &settings,
        &shortcuts,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::hide(app, None)?,
        &PredefinedMenuItem::hide_others(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &restart,
        &quit,
        &stop_all,
    ])?;
    // Copy/paste only work in a macOS webview when the Edit menu has them.
    let edit = Submenu::with_items(app, "Edit", true, &[
        &PredefinedMenuItem::undo(app, None)?,
        &PredefinedMenuItem::redo(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::cut(app, None)?,
        &PredefinedMenuItem::copy(app, None)?,
        &PredefinedMenuItem::paste(app, None)?,
        &PredefinedMenuItem::select_all(app, None)?,
    ])?;
    // No "Close Window" item: ⌘W belongs to the UI (close session).
    let window = Submenu::with_items(app, "Window", true, &[
        &PredefinedMenuItem::minimize(app, None)?,
        &PredefinedMenuItem::fullscreen(app, None)?,
    ])?;
    Menu::with_items(app, &[&roost, &edit, &window])
}

/// Quit: the window goes, every session keeps running in the helper (reopening reconnects).
fn quit(app: &AppHandle) {
    remember_window(app);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        leave(&core);
    }
    app.exit(0);
}

/// The window's size and place, for the next start (also saved on exit; here first, in case).
fn remember_window(app: &AppHandle) {
    let _ = app.save_window_state(StateFlags::all());
}

/// The window goes: sessions stay in the helper, unless Settings says Quit stops them.
fn leave(core: &Core) {
    if core.leaving() {
        return; // already left (a restart keeps every session, whatever Settings says)
    }
    if core.settings().quit_stops_sessions {
        core.shutdown();
    } else {
        core.detach();
    }
}

/// End every session, after a confirmation, and quit.
fn stop_all_and_quit(app: &AppHandle) {
    let core = app.state::<Arc<Core>>().inner().clone();
    let handle = app.clone();
    let n = core.views().iter().filter(|v| v.status != Status::Asleep && v.status != Status::Exited).count();
    let go = move || {
        remember_window(&handle);
        core.shutdown();
        handle.exit(0);
    };
    if n == 0 {
        return go();
    }
    let what = if n == 1 { "1 running session".to_string() } else { format!("{n} running sessions") };
    app.dialog()
        .message(format!("Stop {what} and quit? Anything they're doing ends now."))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Stop and Quit".into(), "Cancel".into()))
        .show(move |yes| {
            if yes {
                go()
            }
        });
}

/// Keeps `~/.local/bin/petramux` pointing at this binary without clobbering anything else.
fn install_cli_link(exe: &Path) {
    let Some(home) = std::env::var_os("HOME") else { return };
    let bin = Path::new(&home).join(".local/bin");
    // The app was called Roost; its link would outlive the old app.
    let old = bin.join("roost");
    if std::fs::read_link(&old).is_ok_and(|t| t.ends_with("Roost.app/Contents/MacOS/roost")) {
        let _ = std::fs::remove_file(&old);
    }
    let link = bin.join("petramux");
    match std::fs::read_link(&link) {
        Ok(target) if target == exe => return,
        Ok(target) if target.file_name() == exe.file_name() => {
            let _ = std::fs::remove_file(&link);
        }
        Ok(_) => {
            eprintln!("petramux: ~/.local/bin/petramux points somewhere else; leaving it");
            return;
        }
        Err(_) if link.symlink_metadata().is_ok() => {
            eprintln!("petramux: ~/.local/bin/petramux exists and isn't a symlink; leaving it");
            return;
        }
        Err(_) => {}
    }
    let _ = std::fs::create_dir_all(&bin);
    if let Err(e) = std::os::unix::fs::symlink(exe, &link) {
        eprintln!("petramux: could not link the CLI: {e}");
    }
}

pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // the window opens where and as big as it was
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .menu(menu)
        .on_menu_event(|app, e| {
            match e.id().as_ref() {
                "quit" => quit(app),
                "stop_all" => stop_all_and_quit(app),
                "restart" => {
                    if let Err(e) = restart_app(app) {
                        eprintln!("petramux: could not restart: {e}");
                    }
                }
                // the window draws these: Settings and the shortcut list
                "settings" | "shortcuts" => {
                    let _ = app.emit("menu", e.id().as_ref());
                }
                _ => {}
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            update::watch_releases(handle.clone());
            if let Err(e) = crate::paths::migrate(&home()) {
                eprintln!("petramux: could not move the Roost folder: {e}");
            }
            let paths = Paths::new(Paths::default_dir());
            let exe = std::env::current_exe()?;
            install_cli_link(&exe);
            // ponytail: blocks startup while the login shell runs (5 s worst case); move to a thread if it's noticeable.
            let env = shellenv::capture_login_env();
            let claude = shellenv::find_claude(&env, std::env::var("ROOST_CLAUDE_BIN").ok());
            let shell = std::env::var_os("ROOST_SHELL")
                .map(PathBuf::from)
                .or_else(|| env.get("SHELL").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
            let sink = Arc::new(UiSink {
                app: handle,
                outputs: Default::default(),
                visible: Default::default(),
            });
            let core = Core::new(Setup { paths: paths.clone(), env, claude, roost_bin: exe.clone(), shell }, sink.clone());
            let (layout, focus) = core.layout();
            *sink.visible.lock().unwrap() = visible_panes(layout.as_ref(), focus.as_deref());
            sock::serve(&paths.sock(), core.handler(), None)?;
            app.manage(core);
            app.manage(sink);
            Ok(())
        })
        .on_window_event(|w, e| match e {
            WindowEvent::CloseRequested { api, .. } if w.label() == "main" => {
                api.prevent_close();
                quit(w.app_handle());
            }
            // a session's own window: the session goes back to the main window
            WindowEvent::CloseRequested { .. } => {
                if let Some(id) = w.label().strip_prefix("s-") {
                    let _ = w.app_handle().emit_to("main", "window_closed", id);
                }
            }
            WindowEvent::Destroyed => {
                if let Some(sink) = w.app_handle().try_state::<Arc<UiSink>>() {
                    sink.outputs.lock().unwrap().drop_window(w.label());
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            boot, attach, select_session, new_session, write, resize, rename, fork, close,
            save_layout, chats, chat_log, chat_say, rename_chat, list_past, resume_past, restart_claude, reopen_closed, pick_folder, ask, diff, appearance, themes, theme, save_drop, set_claude_theme, set_settings, open_link, existing_paths, update_ready, restart, place, open_window, focus_window, close_window
        ])
        .build(tauri::generate_context!())
        .expect("error while building petramux")
        .run(|app, event| match event {
            // `code: None` = the last window went away without our exit(0); `Some` = our own exit(0).
            RunEvent::ExitRequested { api, code: None, .. } => {
                api.prevent_exit();
                quit(app);
            }
            // Dock Quit, logout and shutdown arrive here directly: the sessions stay in the helper.
            RunEvent::Exit => {
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    leave(&core);
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_lets_the_page_handle_drag_and_drop() {
        // Tauri's file-drop handler claims every drag on macOS (returns true to wry), so
        // HTML5 dragover/drop never reach the page and panes can't be docked by dragging.
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["app"]["windows"][0]["dragDropEnabled"], Value::Bool(false));
    }

    #[test]
    fn the_visible_panes_are_the_focused_workspace() {
        let layout = serde_json::json!({"spaces": [
            {"split": "row", "ratio": 0.5, "a": {"pane": "x"}, "b": {"pane": "y"}},
            {"pane": "z"}
        ]});
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert_eq!(visible_panes(Some(&layout), Some("y")), set(&["x", "y"]));
        assert_eq!(visible_panes(Some(&layout), Some("z")), set(&["z"]));
        assert!(visible_panes(Some(&layout), None).is_empty());
        // a layout saved before workspaces is one tree
        let old = serde_json::json!({"split": "row", "ratio": 0.5, "a": {"pane": "x"}, "b": {"pane": "y"}});
        assert_eq!(visible_panes(Some(&old), Some("x")), set(&["x", "y"]));
    }

    #[test]
    fn pane_ids_walks_the_tree() {
        let tree = serde_json::json!({"split": "row", "ratio": 0.5, "a": {"pane": "x"},
            "b": {"split": "col", "ratio": 0.5, "a": {"pane": "y"}, "b": {"pane": "z"}}});
        let ids = pane_ids(Some(&tree));
        assert_eq!(ids, ["x", "y", "z"].into_iter().map(String::from).collect());
        assert!(pane_ids(None).is_empty());
    }
}
