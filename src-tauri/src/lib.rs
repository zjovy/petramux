pub mod paths;
pub mod pty;
pub mod shellenv;
pub mod sock;
pub mod state;

pub fn run_app() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Roost");
}
