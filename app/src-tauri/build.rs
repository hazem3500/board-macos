fn main() {
    // The app's own commands, declared so the ACL checks them: capabilities/quick.json grants them to the Quick add
    // panel only, so the Board page in the main window can't call them (security review change 2).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["panel_info", "panel_hide", "panel_submit", "panel_open_board"]),
    ))
    .expect("failed to run tauri-build");
}
