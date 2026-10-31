//! Runs the import button's work into a folder of your choice, and lists what it found:
//! `cargo run -p horizon-client-render --example import_packs -- <folder>`
use horizon_client_render::{logging, pack_import, packs};
use std::path::PathBuf;

fn main() {
    let dest = PathBuf::from(std::env::args().nth(1).expect("a folder to import into"));
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let folders = pack_import::known_game_folders(&home, &logging::data_dir());
    println!("{} game folders", folders.len());
    println!("{} packs", pack_import::find_packs(&folders, &dest).len());
    for (version, jar) in pack_import::find_version_jars(&folders) {
        println!("version {version}: {}", jar.display());
    }
    let summary = pack_import::import_all(
        &folders,
        &dest,
        horizon_core::VANILLA_VERSION,
        packs::RESOURCE_FORMAT,
    );
    println!("{summary:?}");
}
