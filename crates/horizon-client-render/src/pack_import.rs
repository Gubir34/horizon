//! Brings a player's textures into `resourcepacks/`: packs from other Minecraft launchers' folders, a
//! pack dropped on the window, and the textures of a Minecraft version they already have, made into a
//! pack. Horizon-only (docs/resource-packs.md). Everything stays on the player's computer, is copied
//! and never moved, and nothing already in `resourcepacks/` is overwritten.
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

/// A folder is in progress under this name, so a failure leaves no half a pack in the list.
const PARTIAL: &str = ".importing-";

/// What became of one pack.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Imported(PathBuf),
    /// `resourcepacks/` already has one of that name.
    AlreadyThere,
    /// It holds no `pack.mcmeta`, or no textures.
    NotAPack,
}

/// What one import did altogether.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub imported: usize,
    pub already_there: usize,
    pub failed: usize,
}

impl Summary {
    pub fn add(&mut self, result: &io::Result<Outcome>) {
        match result {
            Ok(Outcome::Imported(_)) => self.imported += 1,
            Ok(Outcome::AlreadyThere) => self.already_there += 1,
            Ok(Outcome::NotAPack) | Err(_) => self.failed += 1,
        }
    }
}

/// The game folders other launchers keep on this computer: the official one, SKLauncher, Prism,
/// MultiMC, the Modrinth App and CurseForge, with each one's instances. `home` is the user's home and
/// `data` the platform's data folder. Only those that exist.
pub fn known_game_folders(home: &Path, data: &Path) -> Vec<PathBuf> {
    let children = |dir: PathBuf| -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    };
    let mut out = vec![
        home.join(".minecraft"),
        data.join(".minecraft"),
        data.join("minecraft"),
        home.join(".sklauncher"),
    ];
    out.extend(children(home.join(".minecraft/modpacks")));
    out.extend(children(home.join(".sklauncher/instances")));
    out.extend(children(data.join("ModrinthApp/profiles")));
    for root in [
        data.join("PrismLauncher/instances"),
        data.join("multimc/instances"),
        home.join(".local/share/multimc/instances"),
    ] {
        for instance in children(root) {
            out.push(instance.join(".minecraft"));
            out.push(instance.join("minecraft"));
        }
    }
    out.extend(children(home.join("curseforge/minecraft/Instances")));
    out.retain(|dir| dir.is_dir());
    out.sort();
    out.dedup();
    out
}

/// Every pack in those folders' `resourcepacks/`, once per file or folder name: the same pack is often
/// in several instances, and the first found stands. `own` is left out, however it is reached.
pub fn find_packs(folders: &[PathBuf], own: &Path) -> Vec<PathBuf> {
    let own = fs::canonicalize(own).unwrap_or_else(|_| own.to_owned());
    let mut names = BTreeSet::new();
    let mut out = Vec::new();
    for folder in folders {
        let dir = folder.join("resourcepacks");
        if fs::canonicalize(&dir).is_ok_and(|d| d == own) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if is_pack(&path)
                && let Some(name) = path.file_name()
                && names.insert(name.to_owned())
            {
                out.push(path);
            }
        }
    }
    out
}

/// A version's name and client jar, from each folder's `versions/`. A modded profile's jar usually
/// holds no textures of its own, and then [`textures_from_jar`] says so.
pub fn find_version_jars(folders: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for folder in folders {
        let Ok(entries) = fs::read_dir(folder.join("versions")) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for version in names {
            let jar = folder
                .join("versions")
                .join(&version)
                .join(format!("{version}.jar"));
            if jar.is_file() && seen.insert(version.clone()) {
                out.push((version, jar));
            }
        }
    }
    out
}

/// Whether `path` is a resource pack: a folder or a zip with a `pack.mcmeta` at its root.
pub fn is_pack(path: &Path) -> bool {
    if path.is_dir() {
        return path.join("pack.mcmeta").is_file();
    }
    if !has_zip_name(path) {
        return false;
    }
    fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .is_some_and(|mut z| z.by_name("pack.mcmeta").is_ok())
}

fn has_zip_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Copies the pack at `source` into `packs_dir` under its own name.
pub fn import_pack(source: &Path, packs_dir: &Path) -> io::Result<Outcome> {
    if !is_pack(source) {
        return Ok(Outcome::NotAPack);
    }
    let Some(name) = source.file_name() else {
        return Ok(Outcome::NotAPack);
    };
    let target = packs_dir.join(name);
    if target.exists() {
        return Ok(Outcome::AlreadyThere);
    }
    fs::create_dir_all(packs_dir)?;
    let partial = packs_dir.join(format!("{PARTIAL}{}", name.to_string_lossy()));
    remove(&partial)?;
    let copied = if source.is_dir() {
        copy_folder(source, &partial)
    } else {
        fs::copy(source, &partial).map(|_| ())
    };
    if let Err(e) = copied.and_then(|()| fs::rename(&partial, &target)) {
        let _ = remove(&partial);
        return Err(e);
    }
    Ok(Outcome::Imported(target))
}

fn remove(path: &Path) -> io::Result<()> {
    let removed = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Files and folders only: a symbolic link in a pack could reach anything on the computer.
fn copy_folder(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Whether a jar's entry belongs in a pack of its textures: an image or its animation under some
/// namespace's `textures/`.
fn is_texture(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("assets/") else {
        return false;
    };
    let Some((_namespace, path)) = rest.split_once('/') else {
        return false;
    };
    path.starts_with("textures/") && (name.ends_with(".png") || name.ends_with(".png.mcmeta"))
}

/// The folder a version's textures go in: a version name can hold what a folder's can't.
pub fn textures_pack_name(version: &str) -> String {
    let safe: String = version
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "._- ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("Minecraft {} textures", safe.trim())
}

/// Makes a pack of the textures in the player's own client jar, for `format`: the look of another
/// version, directly from the game. Textures whose paths changed between versions are simply not used.
pub fn textures_from_jar(
    jar: &Path,
    version: &str,
    packs_dir: &Path,
    format: (i64, i64),
) -> io::Result<Outcome> {
    let name = textures_pack_name(version);
    let target = packs_dir.join(&name);
    if target.exists() {
        return Ok(Outcome::AlreadyThere);
    }
    let mut archive = zip::ZipArchive::new(fs::File::open(jar)?).map_err(io::Error::other)?;
    let partial = packs_dir.join(format!("{PARTIAL}{name}"));
    remove(&partial)?;
    fs::create_dir_all(&partial)?;
    let written = extract_textures(&mut archive, &partial).and_then(|count| {
        if count > 0 {
            write_mcmeta(&partial, version, format)?;
            fs::rename(&partial, &target)?;
        }
        Ok(count)
    });
    match written {
        Ok(0) => {
            remove(&partial)?;
            Ok(Outcome::NotAPack)
        }
        Ok(_) => Ok(Outcome::Imported(target)),
        Err(e) => {
            let _ = remove(&partial);
            Err(e)
        }
    }
}

fn extract_textures(archive: &mut zip::ZipArchive<fs::File>, to: &Path) -> io::Result<usize> {
    let mut count = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        if !entry.is_file() || !is_texture(entry.name()) {
            continue;
        }
        // `enclosed_name` refuses `..` and absolute paths: a jar is a file like any other.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let path = to.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut fs::File::create(path)?)?;
        count += 1;
    }
    Ok(count)
}

fn write_mcmeta(dir: &Path, version: &str, format: (i64, i64)) -> io::Result<()> {
    let meta = serde_json::json!({
        "pack": {
            "description": format!("Textures of Minecraft {version}, from your own copy"),
            "min_format": [format.0, format.1],
            "max_format": [format.0, format.1],
        }
    });
    fs::write(dir.join("pack.mcmeta"), meta.to_string())
}

/// Imports every pack the other launchers' folders hold, and a pack of textures from each version
/// other than `current`, whose textures the game already has.
pub fn import_all(
    folders: &[PathBuf],
    packs_dir: &Path,
    current: &str,
    format: (i64, i64),
) -> Summary {
    let mut summary = Summary::default();
    for pack in find_packs(folders, packs_dir) {
        let result = import_pack(&pack, packs_dir);
        if let Err(e) = &result {
            log::warn!("Horizon: could not import {}: {e}", pack.display());
        }
        summary.add(&result);
    }
    for (version, jar) in find_version_jars(folders) {
        if version == current {
            continue;
        }
        match textures_from_jar(&jar, &version, packs_dir, format) {
            // A modded profile's jar, or a launcher's own: there was nothing to take.
            Ok(Outcome::NotAPack) => {}
            result => {
                if let Err(e) = &result {
                    log::warn!("Horizon: could not read {}: {e}", jar.display());
                }
                summary.add(&result);
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FORMAT: (i64, i64) = (88, 0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("horizon-pack-import-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn folder_pack(path: &Path) {
        fs::create_dir_all(path.join("assets/minecraft/textures/block")).unwrap();
        fs::write(path.join("pack.mcmeta"), "{}").unwrap();
        fs::write(
            path.join("assets/minecraft/textures/block/stone.png"),
            "png",
        )
        .unwrap();
    }

    #[test]
    fn a_folder_and_a_zip_copy_over_and_the_originals_stay() {
        let t = Temp::new("copy");
        let dest = t.0.join("dest");
        let folder = t.0.join("Folder");
        folder_pack(&folder);
        let zip = t.0.join("Zipped.zip");
        write_zip(&zip, &[("pack.mcmeta", b"{}")]);
        assert_eq!(
            import_pack(&folder, &dest).unwrap(),
            Outcome::Imported(dest.join("Folder"))
        );
        assert_eq!(
            import_pack(&zip, &dest).unwrap(),
            Outcome::Imported(dest.join("Zipped.zip"))
        );
        assert!(
            dest.join("Folder/assets/minecraft/textures/block/stone.png")
                .is_file()
        );
        assert!(is_pack(&dest.join("Zipped.zip")));
        assert!(folder.is_dir() && zip.is_file());
    }

    #[test]
    fn what_is_already_there_is_never_overwritten() {
        let t = Temp::new("keep");
        let dest = t.0.join("dest");
        let zip = t.0.join("P.zip");
        write_zip(&zip, &[("pack.mcmeta", b"new")]);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("P.zip"), "mine").unwrap();
        assert_eq!(import_pack(&zip, &dest).unwrap(), Outcome::AlreadyThere);
        assert_eq!(fs::read(dest.join("P.zip")).unwrap(), b"mine");
    }

    #[test]
    fn what_isnt_a_pack_is_refused() {
        let t = Temp::new("refuse");
        let dest = t.0.join("dest");
        let plain = t.0.join("photo.png");
        fs::write(&plain, "png").unwrap();
        let empty = t.0.join("Empty.zip");
        write_zip(&empty, &[("readme.txt", b"hi")]);
        let broken = t.0.join("Broken.zip");
        fs::write(&broken, "not a zip").unwrap();
        let bare = t.0.join("Bare");
        fs::create_dir_all(&bare).unwrap();
        for source in [&plain, &empty, &broken, &bare] {
            assert_eq!(import_pack(source, &dest).unwrap(), Outcome::NotAPack);
        }
        assert!(!dest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_a_pack_is_left_behind() {
        let t = Temp::new("link");
        let secret = t.0.join("secret.txt");
        fs::write(&secret, "private").unwrap();
        let pack = t.0.join("Linky");
        folder_pack(&pack);
        std::os::unix::fs::symlink(&secret, pack.join("assets/leak.txt")).unwrap();
        let dest = t.0.join("dest");
        import_pack(&pack, &dest).unwrap();
        assert!(!dest.join("Linky/assets/leak.txt").exists());
        assert!(dest.join("Linky/pack.mcmeta").is_file());
    }

    #[test]
    fn packs_are_found_once_and_our_own_folder_is_skipped() {
        let t = Temp::new("find");
        let one = t.0.join("one");
        let two = t.0.join("two");
        folder_pack(&one.join("resourcepacks/Shared"));
        folder_pack(&two.join("resourcepacks/Shared"));
        write_zip(
            &two.join("resourcepacks/Only.zip"),
            &[("pack.mcmeta", b"{}")],
        );
        fs::create_dir_all(one.join("resourcepacks/NotAPack")).unwrap();
        let own = t.0.join("own");
        folder_pack(&own.join("resourcepacks/Mine"));
        let folders = [one.clone(), two.clone(), own.clone()];
        let found = find_packs(&folders, &own.join("resourcepacks"));
        assert_eq!(
            found,
            [
                one.join("resourcepacks/Shared"),
                two.join("resourcepacks/Only.zip")
            ]
        );
    }

    #[test]
    fn a_jars_textures_become_a_pack_of_this_format() {
        let t = Temp::new("jar");
        let jar = t.0.join("1.21.jar");
        write_zip(
            &jar,
            &[
                ("assets/minecraft/textures/block/stone.png", b"png"),
                ("assets/minecraft/textures/block/lava.png.mcmeta", b"{}"),
                ("assets/minecraft/models/block/stone.json", b"{}"),
                ("assets/minecraft/lang/en_us.json", b"{}"),
                ("net/minecraft/Main.class", b"code"),
            ],
        );
        let dest = t.0.join("dest");
        let pack = dest.join("Minecraft 1.21 textures");
        assert_eq!(
            textures_from_jar(&jar, "1.21", &dest, FORMAT).unwrap(),
            Outcome::Imported(pack.clone())
        );
        assert!(
            pack.join("assets/minecraft/textures/block/stone.png")
                .is_file()
        );
        assert!(
            pack.join("assets/minecraft/textures/block/lava.png.mcmeta")
                .is_file()
        );
        assert!(!pack.join("assets/minecraft/models").exists());
        assert!(!pack.join("net").exists());
        assert!(is_pack(&pack));
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(pack.join("pack.mcmeta")).unwrap()).unwrap();
        assert_eq!(meta["pack"]["min_format"], serde_json::json!([88, 0]));
        assert_eq!(
            textures_from_jar(&jar, "1.21", &dest, FORMAT).unwrap(),
            Outcome::AlreadyThere
        );
    }

    #[test]
    fn a_jar_without_textures_leaves_nothing() {
        let t = Temp::new("modded");
        let jar = t.0.join("fabric.jar");
        write_zip(&jar, &[("net/fabricmc/Knot.class", b"code")]);
        let dest = t.0.join("dest");
        assert_eq!(
            textures_from_jar(&jar, "fabric", &dest, FORMAT).unwrap(),
            Outcome::NotAPack
        );
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn a_jar_cant_write_outside_the_pack() {
        let t = Temp::new("escape");
        let jar = t.0.join("evil.jar");
        write_zip(
            &jar,
            &[
                ("assets/minecraft/textures/../../../../escaped.png", b"png"),
                ("assets/minecraft/textures/block/fine.png", b"png"),
            ],
        );
        let dest = t.0.join("dest");
        textures_from_jar(&jar, "evil", &dest, FORMAT).unwrap();
        assert!(!t.0.join("escaped.png").exists());
        assert!(!dest.join("escaped.png").exists());
        assert!(
            dest.join("Minecraft evil textures/assets/minecraft/textures/block/fine.png")
                .is_file()
        );
    }

    #[test]
    fn a_version_name_cant_leave_the_folder() {
        assert_eq!(textures_pack_name("1.21"), "Minecraft 1.21 textures");
        assert_eq!(textures_pack_name("../../x"), "Minecraft .._.._x textures");
    }

    #[test]
    fn import_all_takes_packs_and_other_versions_only() {
        let t = Temp::new("all");
        let game = t.0.join("game");
        folder_pack(&game.join("resourcepacks/Cool"));
        let textures: &[(&str, &[u8])] = &[("assets/minecraft/textures/block/a.png", b"png")];
        write_zip(&game.join("versions/26.3/26.3.jar"), textures);
        write_zip(&game.join("versions/1.21/1.21.jar"), textures);
        write_zip(
            &game.join("versions/fabric/fabric.jar"),
            &[("a.class", b"c")],
        );
        let dest = t.0.join("dest");
        let folders = [game];
        let summary = import_all(&folders, &dest, "26.3", FORMAT);
        assert_eq!(
            summary,
            Summary {
                imported: 2,
                already_there: 0,
                failed: 0
            }
        );
        assert!(dest.join("Cool").is_dir());
        assert!(dest.join("Minecraft 1.21 textures").is_dir());
        assert!(!dest.join("Minecraft 26.3 textures").exists());
        let again = import_all(&folders, &dest, "26.3", FORMAT);
        assert_eq!((again.imported, again.already_there), (0, 2));
    }
}
