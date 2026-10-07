//! Build script.
//!
//! Beyond the default Tauri setup, this declares the icon files as build inputs.
//!
//! That declaration is not cosmetic. `tauri-build` compiles the icons into a
//! Windows resource, and cargo caches the result of this script. Without a
//! `rerun-if-changed` entry for the icon files, replacing `icons/icon.ico`
//! leaves the cache valid, so the next build reuses the **previously compiled**
//! resource and the executable keeps the old artwork.
//!
//! That is exactly what happened here: the logo was replaced, the icon set was
//! regenerated, and every subsequent build still embedded the earlier icon. The
//! only symptom was visual, so nothing failed: the app built, installed, and ran
//! with the wrong icon in the title bar, taskbar, Desktop shortcut, and Start
//! Menu. It was found by extracting the embedded PNGs and comparing their pixel
//! data against `src-tauri/icons`, not by reading any configuration.
//!
//! Declaring the inputs makes cargo rerun this script whenever an icon changes,
//! which recompiles the resource.

use std::path::Path;

fn main() {
    // Every icon the bundle can reference, plus the whole directory, so a newly
    // generated size is picked up without editing this list.
    let icons = Path::new("icons");
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    if let Ok(entries) = std::fs::read_dir(icons) {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }

    tauri_build::build()
}
