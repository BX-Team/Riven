use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{LaunchError, io};

/// A world in an instance's `saves/`.
#[derive(Debug, Clone)]
pub struct World {
    /// The folder name under `saves/`.
    pub folder: String,
    /// The name the game shows, from `level.dat`; the folder name when it has none.
    pub name: String,
    pub icon: Option<PathBuf>,
    pub mode: Option<GameMode>,
    pub difficulty: Option<Difficulty>,
    pub cheats: bool,
    pub seed: Option<i64>,
    /// In-game days since the world was made.
    pub days: Option<u64>,
    /// The Minecraft version that saved it last.
    pub version: Option<String>,
    pub last_played: SystemTime,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
    Hardcore,
}

/// A zipped copy of a world, kept in the instance folder out of the game's sight.
#[derive(Debug, Clone)]
pub struct Backup {
    pub path: PathBuf,
    pub created: SystemTime,
    pub size: u64,
}

const BACKUPS: &str = "backups/worlds";

/// The worlds of a game folder, last played first.
pub fn list(game_dir: &Path) -> Vec<World> {
    let Ok(entries) = std::fs::read_dir(game_dir.join("saves")) else {
        return Vec::new();
    };
    let mut worlds: Vec<World> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let folder = e.file_name().to_str()?.to_owned();
            if folder.starts_with('.') || !path.join("level.dat").is_file() {
                return None;
            }
            Some(read_world(&path, folder))
        })
        .collect();
    worlds.sort_by_key(|w| std::cmp::Reverse(w.last_played));
    worlds
}

fn read_world(path: &Path, folder: String) -> World {
    let level = std::fs::read(path.join("level.dat"))
        .ok()
        .and_then(|bytes| read_level(&bytes))
        .unwrap_or_default();
    let modified = std::fs::metadata(path.join("level.dat"))
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let last_played = level
        .last_played
        .and_then(|ms| u64::try_from(ms).ok())
        .map_or(modified, |ms| {
            SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
        });
    let icon = Some(path.join("icon.png")).filter(|p| p.is_file());
    World {
        name: level
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| folder.clone()),
        folder,
        icon,
        mode: level.mode,
        difficulty: level.difficulty,
        cheats: level.cheats,
        seed: level.seed,
        days: level
            .time
            .and_then(|t| u64::try_from(t).ok())
            .map(|t| t / 24_000 + 1),
        version: level.version,
        last_played,
        size: dir_size(path),
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// `saves/<folder>`, refusing names that would leave `saves/`.
fn world_dir(game_dir: &Path, folder: &str) -> Result<PathBuf, LaunchError> {
    let plain =
        !folder.is_empty() && folder != "." && folder != ".." && !folder.contains(['/', '\\', ':']);
    if !plain {
        return Err(LaunchError::Io {
            path: PathBuf::from(folder),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a world folder"),
        });
    }
    Ok(game_dir.join("saves").join(folder))
}

fn backups_dir(instance_dir: &Path, folder: &str) -> PathBuf {
    instance_dir.join(BACKUPS).join(folder)
}

/// The backups of one world, newest first.
pub fn backups(instance_dir: &Path, folder: &str) -> Vec<Backup> {
    let Ok(entries) = std::fs::read_dir(backups_dir(instance_dir, folder)) else {
        return Vec::new();
    };
    let mut out: Vec<Backup> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(Backup {
                path: e.path(),
                created: meta.modified().ok()?,
                size: meta.len(),
            })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.created));
    out
}

/// Zips a world into the instance's backups; the game may keep running.
pub fn backup(instance_dir: &Path, game_dir: &Path, folder: &str) -> Result<Backup, LaunchError> {
    let world = world_dir(game_dir, folder)?;
    let dir = backups_dir(instance_dir, folder);
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let out = (0..)
        .map(|n| match n {
            0 => dir.join(format!("{stamp}.zip")),
            n => dir.join(format!("{stamp}-{n}.zip")),
        })
        .find(|p| !p.exists())
        .expect("some suffix is free");
    let tmp = out.with_extension("zip.part");
    let written = write_zip(&world, &tmp);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &out).map_err(io(&out))?;
    let size = std::fs::metadata(&out).map_or(0, |m| m.len());
    Ok(Backup {
        path: out,
        created: SystemTime::now(),
        size,
    })
}

fn write_zip(world: &Path, out: &Path) -> Result<(), LaunchError> {
    let file = std::fs::File::create(out).map_err(io(out))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let zip_err = |e: zip::result::ZipError| LaunchError::Io {
        path: out.to_owned(),
        source: std::io::Error::other(e),
    };
    let mut stack = vec![(world.to_owned(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(io(&dir))? {
            let entry = entry.map_err(io(&dir))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            let kind = entry.file_type().map_err(io(&path))?;
            let inner = format!("{prefix}{name}");
            if kind.is_dir() {
                zip.add_directory(&inner, options).map_err(zip_err)?;
                stack.push((path, format!("{inner}/")));
            } else if kind.is_file() && name != "session.lock" {
                zip.start_file(&inner, options).map_err(zip_err)?;
                let mut src = std::fs::File::open(&path).map_err(io(&path))?;
                std::io::copy(&mut src, &mut zip).map_err(io(&path))?;
            }
        }
    }
    zip.finish().map_err(zip_err)?.flush().map_err(io(out))
}

/// Puts a backup in place of the world; the current one is dropped only once the backup is unpacked.
pub fn restore(game_dir: &Path, folder: &str, backup: &Path) -> Result<(), LaunchError> {
    let world = world_dir(game_dir, folder)?;
    let saves = game_dir.join("saves");
    let fresh = saves.join(format!(".{folder}.restoring"));
    let old = saves.join(format!(".{folder}.old"));
    for stale in [&fresh, &old] {
        if stale.exists() {
            std::fs::remove_dir_all(stale).map_err(io(stale))?;
        }
    }
    if let Err(e) = unzip(backup, &fresh) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(e);
    }
    if world.exists() {
        std::fs::rename(&world, &old).map_err(io(&world))?;
    }
    std::fs::rename(&fresh, &world).map_err(io(&world))?;
    if old.exists() {
        std::fs::remove_dir_all(&old).map_err(io(&old))?;
    }
    Ok(())
}

fn unzip(archive: &Path, into: &Path) -> Result<(), LaunchError> {
    let file = std::fs::File::open(archive).map_err(io(archive))?;
    let zip_err = |e: zip::result::ZipError| LaunchError::Io {
        path: archive.to_owned(),
        source: std::io::Error::other(e),
    };
    let mut zip = zip::ZipArchive::new(file).map_err(zip_err)?;
    std::fs::create_dir_all(into).map_err(io(into))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(zip_err)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = into.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(io(&out))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        let mut dst = std::fs::File::create(&out).map_err(io(&out))?;
        std::io::copy(&mut entry, &mut dst).map_err(io(&out))?;
    }
    Ok(())
}

/// Copies a world into another game folder under a free folder name, which it returns.
pub fn copy_to(game_dir: &Path, folder: &str, to_game_dir: &Path) -> Result<String, LaunchError> {
    let from = world_dir(game_dir, folder)?;
    let saves = to_game_dir.join("saves");
    let name = (1..)
        .map(|n| match n {
            1 => folder.to_owned(),
            n => format!("{folder} ({n})"),
        })
        .find(|n| !saves.join(n).exists())
        .expect("some suffix is free");
    copy_dir(&from, &saves.join(&name))?;
    Ok(name)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), LaunchError> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(io(&src))?;
        if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else if kind.is_file() && entry.file_name() != "session.lock" {
            std::fs::copy(&src, &dst).map_err(io(&src))?;
        }
    }
    Ok(())
}

pub fn delete(game_dir: &Path, folder: &str) -> Result<(), LaunchError> {
    let world = world_dir(game_dir, folder)?;
    std::fs::remove_dir_all(&world).map_err(io(&world))
}

pub fn delete_backup(backup: &Path) -> Result<(), LaunchError> {
    std::fs::remove_file(backup).map_err(io(backup))
}

#[derive(Debug, Default, PartialEq)]
struct Level {
    name: Option<String>,
    last_played: Option<i64>,
    mode: Option<GameMode>,
    difficulty: Option<Difficulty>,
    cheats: bool,
    seed: Option<i64>,
    time: Option<i64>,
    version: Option<String>,
}

/// What the launcher shows from a gzipped `level.dat`: the `Data` compound's few fields.
fn read_level(gzipped: &[u8]) -> Option<Level> {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(gzipped)
        .read_to_end(&mut bytes)
        .ok()?;
    let mut r = Nbt { bytes: &bytes };
    if r.u8()? != COMPOUND {
        return None;
    }
    r.string()?;
    let mut level = Level::default();
    let (mut game_type, mut hardcore) = (None, false);
    r.compound(&mut |r, kind, name| match (name, kind) {
        ("Data", COMPOUND) => r.compound(&mut |r, kind, name| match (name, kind) {
            ("LevelName", STRING) => {
                level.name = Some(r.string()?);
                Some(())
            }
            ("LastPlayed", LONG) => {
                level.last_played = Some(r.i64()?);
                Some(())
            }
            ("GameType", INT) => {
                game_type = Some(r.i32()?);
                Some(())
            }
            ("hardcore", BYTE) => {
                hardcore = r.u8()? != 0;
                Some(())
            }
            ("allowCommands", BYTE) => {
                level.cheats = r.u8()? != 0;
                Some(())
            }
            ("Difficulty", BYTE) => {
                level.difficulty = match r.u8()? {
                    0 => Some(Difficulty::Peaceful),
                    1 => Some(Difficulty::Easy),
                    2 => Some(Difficulty::Normal),
                    3 => Some(Difficulty::Hard),
                    _ => None,
                };
                Some(())
            }
            ("Time", LONG) => {
                level.time = Some(r.i64()?);
                Some(())
            }
            ("RandomSeed", LONG) => {
                level.seed = level.seed.or(Some(r.i64()?));
                Some(())
            }
            ("WorldGenSettings", COMPOUND) => r.compound(&mut |r, kind, name| match (name, kind) {
                ("seed", LONG) => {
                    level.seed = Some(r.i64()?);
                    Some(())
                }
                _ => r.skip(kind),
            }),
            ("Version", COMPOUND) => r.compound(&mut |r, kind, name| match (name, kind) {
                ("Name", STRING) => {
                    level.version = Some(r.string()?);
                    Some(())
                }
                _ => r.skip(kind),
            }),
            _ => r.skip(kind),
        }),
        _ => r.skip(kind),
    })?;
    level.mode = match (hardcore, game_type) {
        (true, _) => Some(GameMode::Hardcore),
        (_, Some(0)) => Some(GameMode::Survival),
        (_, Some(1)) => Some(GameMode::Creative),
        (_, Some(2)) => Some(GameMode::Adventure),
        (_, Some(3)) => Some(GameMode::Spectator),
        _ => None,
    };
    Some(level)
}

const END: u8 = 0;
const BYTE: u8 = 1;
const INT: u8 = 3;
const LONG: u8 = 4;
const STRING: u8 = 8;
const LIST: u8 = 9;
const COMPOUND: u8 = 10;

/// A big-endian NBT reader over the uncompressed bytes.
struct Nbt<'a> {
    bytes: &'a [u8],
}

impl Nbt<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.bytes.len() < n {
            return None;
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Java's modified UTF-8 matches UTF-8 for everything a world name holds in practice.
    fn string(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        Some(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    /// Reads tags until `End`, handing each one's kind and name to `each`, which must consume its payload.
    fn compound(&mut self, each: &mut dyn FnMut(&mut Self, u8, &str) -> Option<()>) -> Option<()> {
        loop {
            let kind = self.u8()?;
            if kind == END {
                return Some(());
            }
            let name = self.string()?;
            each(self, kind, &name)?;
        }
    }

    fn skip(&mut self, kind: u8) -> Option<()> {
        let len = |r: &mut Self| usize::try_from(r.i32()?).ok();
        match kind {
            1 => self.take(1).map(drop),
            2 => self.take(2).map(drop),
            3 | 5 => self.take(4).map(drop),
            4 | 6 => self.take(8).map(drop),
            7 => {
                let n = len(self)?;
                self.take(n).map(drop)
            }
            STRING => self.string().map(drop),
            LIST => {
                let inner = self.u8()?;
                for _ in 0..len(self)? {
                    self.skip(inner)?;
                }
                Some(())
            }
            COMPOUND => self.compound(&mut |r, kind, _| r.skip(kind)),
            11 => {
                let n = len(self)?;
                self.take(n.checked_mul(4)?).map(drop)
            }
            12 => {
                let n = len(self)?;
                self.take(n.checked_mul(8)?).map(drop)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(out: &mut Vec<u8>, kind: u8, name: &str) {
        out.push(kind);
        out.extend((name.len() as u16).to_be_bytes());
        out.extend(name.as_bytes());
    }

    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend((s.len() as u16).to_be_bytes());
        out.extend(s.as_bytes());
    }

    /// A `level.dat` with the fields Riven reads among others it must step over.
    fn level_dat() -> Vec<u8> {
        let mut nbt = Vec::new();
        tag(&mut nbt, COMPOUND, "");
        tag(&mut nbt, COMPOUND, "Data");
        tag(&mut nbt, LIST, "ServerBrands");
        nbt.push(STRING);
        nbt.extend(2i32.to_be_bytes());
        string(&mut nbt, "fabric");
        string(&mut nbt, "vanilla");
        tag(&mut nbt, 12, "Seeds");
        nbt.extend(1i32.to_be_bytes());
        nbt.extend(7i64.to_be_bytes());
        tag(&mut nbt, STRING, "LevelName");
        string(&mut nbt, "Мой мир");
        tag(&mut nbt, COMPOUND, "Version");
        tag(&mut nbt, INT, "Id");
        nbt.extend(3955i32.to_be_bytes());
        tag(&mut nbt, STRING, "Name");
        string(&mut nbt, "1.21.1");
        nbt.push(END);
        tag(&mut nbt, INT, "GameType");
        nbt.extend(1i32.to_be_bytes());
        tag(&mut nbt, BYTE, "Difficulty");
        nbt.push(3);
        tag(&mut nbt, BYTE, "allowCommands");
        nbt.push(1);
        tag(&mut nbt, LONG, "Time");
        nbt.extend(50_000i64.to_be_bytes());
        tag(&mut nbt, COMPOUND, "WorldGenSettings");
        tag(&mut nbt, LONG, "seed");
        nbt.extend((-42i64).to_be_bytes());
        nbt.push(END);
        tag(&mut nbt, LONG, "LastPlayed");
        nbt.extend(1_730_000_000_000i64.to_be_bytes());
        nbt.push(END);
        nbt.push(END);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn level_dat_fields_are_read_past_lists_and_arrays() {
        assert_eq!(
            read_level(&level_dat()),
            Some(Level {
                name: Some("Мой мир".into()),
                last_played: Some(1_730_000_000_000),
                mode: Some(GameMode::Creative),
                difficulty: Some(Difficulty::Hard),
                cheats: true,
                seed: Some(-42),
                time: Some(50_000),
                version: Some("1.21.1".into()),
            })
        );
        assert_eq!(read_level(&level_dat()[..20]), None);
    }

    #[test]
    fn backup_restores_over_the_world_and_ignores_escaping_entries() {
        let root = std::env::temp_dir().join(format!("riven-worlds-{}", std::process::id()));
        let (instance, game) = (root.join("inst"), root.join("inst/minecraft"));
        let world = game.join("saves/w");
        std::fs::create_dir_all(world.join("region")).unwrap();
        std::fs::write(world.join("level.dat"), level_dat()).unwrap();
        std::fs::write(world.join("region/r.0.0.mca"), b"before").unwrap();
        std::fs::write(world.join("session.lock"), b"x").unwrap();

        let saved = backup(&instance, &game, "w").unwrap();
        std::fs::write(world.join("region/r.0.0.mca"), b"after").unwrap();
        std::fs::write(world.join("new.txt"), b"x").unwrap();
        restore(&game, "w", &saved.path).unwrap();
        assert_eq!(
            std::fs::read(world.join("region/r.0.0.mca")).unwrap(),
            b"before"
        );
        assert!(!world.join("new.txt").exists());
        assert!(!world.join("session.lock").exists());
        assert_eq!(list(&game)[0].name, "Мой мир");

        let evil = root.join("evil.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("../../escaped.txt", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.start_file("level.dat", options).unwrap();
        zip.finish().unwrap();
        restore(&game, "w", &evil).unwrap();
        assert!(!game.join("escaped.txt").exists() && !root.join("escaped.txt").exists());
        assert!(world_dir(&game, "../x").is_err());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
