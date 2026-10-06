//! Tauri application bootstrap.
//!
//! Startup rules (PRD §55): detection and local reads are allowed; no provider
//! request, no config mutation, and no terminal window is opened before the
//! user asks for one.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    eclyzer_bridge_lib::run();
}
