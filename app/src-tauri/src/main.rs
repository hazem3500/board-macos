// Board for macOS: see lib.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    board_mac::run()
}
