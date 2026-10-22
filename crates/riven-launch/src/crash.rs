use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::logs;

/// Module names in stack frames that belong to the game, the loader or Java, never to a mod.
const CORE: &[&str] = &[
    "minecraft",
    "neoforge",
    "forge",
    "fml",
    "fmlcore",
    "fmlloader",
    "javafml",
    "lowcodefml",
    "mclanguage",
];

/// Packages of the game, the loaders and Java: a missing class there means the mod targets another version.
const CORE_PACKAGES: &[&str] = &[
    "net.minecraft.",
    "com.mojang.",
    "net.neoforged.",
    "net.minecraftforge.",
    "net.fabricmc.",
    "org.quiltmc.",
    "java.",
    "javax.",
    "jdk.",
    "sun.",
];

/// Messages of a jar that is cut short or is not a jar at all.
const CORRUPT: &[&str] = &[
    "zip END header not found",
    "invalid CEN header",
    "invalid LOC header",
    "zip file is empty",
    "error in opening zip file",
    "Invalid or corrupt jarfile",
    " is not a jar file",
];

/// Messages of a game that could not get an OpenGL window; plain GL errors in a log are harmless.
const GRAPHICS: &[&str] = &[
    "Pixel format not accelerated",
    "does not appear to support OpenGL",
    "Failed to initialize GLFW",
    "GLFW error during init",
    "No OpenGL context",
    "Could not create context",
    "LinuxDisplay.getAvailableDisplayModes",
];

const DISK_FULL: &[&str] = &[
    "No space left on device",
    "There is not enough space on the disk",
];

/// Library name parts of GPU drivers, by vendor.
const DRIVERS: &[(Vendor, &[&str])] = &[
    (
        Vendor::Nvidia,
        &[
            "nvoglv",
            "nvd3dum",
            "nvwgf",
            "nvcuda",
            "libnvidia",
            "glx_nvidia",
        ],
    ),
    (
        Vendor::Amd,
        &[
            "atio6axx", "atioglxx", "atig6", "aticfx", "amdvlk", "amdxx", "radeonsi", "libamd",
        ],
    ),
    (
        Vendor::Intel,
        &[
            "ig7icd",
            "ig75icd",
            "ig8icd",
            "ig9icd",
            "ig11icd",
            "igxelpicd",
            "igdumd",
            "igd10",
            "iris_dri",
            "i965_dri",
            "crocus_dri",
            "libigc",
        ],
    ),
];

/// Library name parts of graphics code with no known vendor: Mesa, the system's GL and Vulkan loaders.
const GRAPHICS_LIBRARIES: &[&str] = &[
    "_dri",
    "libgl.",
    "libglx",
    "libegl",
    "opengl32",
    "libvulkan",
    "vulkan-1",
    "libgallium",
    "d3d",
];

/// Where a piece of text comes from, which decides the rules that trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// A crash report the game wrote.
    Report,
    /// An `hs_err_pid*.log` the JVM wrote when it crashed itself.
    Jvm,
    /// `logs/latest.log`: mods log exceptions they survive, so only loader and JVM errors count.
    Log,
    /// What the game printed, including what Java says before any log exists.
    Output,
}

/// A mod a crash points at; `name` and `file` are filled in from the instance's jars when found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Culprit {
    pub id: String,
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    /// Ships inside another mod's jar, which `file` then is.
    pub nested: bool,
}

impl Culprit {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: None,
            file: None,
            nested: false,
        }
    }

    /// The mod's name when known, else its id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    fn named(mut self, name: &str) -> Self {
        self.name.get_or_insert_with(|| name.to_owned());
        self
    }
}

/// A requirement the loader found unmet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub by: Culprit,
    pub needs: Culprit,
    /// The version range as the loader wrote it.
    pub range: String,
    /// The installed version, `None` when it is missing.
    pub found: Option<String>,
    /// `by` refuses to run with `needs` rather than requiring it.
    pub incompatible: bool,
}

/// A mod id the loader found in more than one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub culprit: Culprit,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
}

/// What the JVM was running when it crashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    /// A graphics driver, by vendor when its library tells.
    Driver(Option<Vendor>),
    /// Java itself.
    Java,
    /// Another native library, by file name.
    Library(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickingKind {
    Entity,
    Block,
}

/// The entity or block the game was updating when it crashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticking {
    pub kind: TickingKind,
    /// `minecraft:zombie`, `create:mechanical_press`.
    pub id: String,
    /// Block coordinates as `x, y, z`.
    pub at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The loader refused to start the game.
    Dependencies(Vec<Dependency>),
    /// The loader refused to start the game: the same mod is installed more than once.
    Duplicates(Vec<Duplicate>),
    /// Two mods ship the same Java package.
    Overlap {
        first: Culprit,
        second: Culprit,
        package: String,
    },
    /// `culprit` calls code that is not there: of `other`, or of the game when `None`.
    Mismatch {
        culprit: Culprit,
        other: Option<Culprit>,
    },
    /// Code some mod needs is not installed, and no mod in the stack tells which.
    MissingClass {
        class: String,
    },
    /// A mod could not patch the game.
    Mixin(Culprit),
    /// A mod crashed while the loader was starting it.
    Load {
        culprit: Culprit,
        error: String,
    },
    /// The game was started with an older Java than it needs.
    Java {
        needed: u32,
    },
    /// The game, made for Java 8, was started with a newer one.
    JavaTooNew,
    /// The game ran out of its Java memory.
    Memory,
    /// The computer ran out of memory for Java.
    SystemMemory,
    /// Java could not set aside the memory the settings ask for.
    HeapSize,
    /// The JVM itself crashed.
    Native(Native),
    /// The game could not open an OpenGL window.
    Graphics,
    DiskFull,
    /// A jar is cut short or is not a jar; `file` is its name when the message tells.
    Corrupt {
        file: Option<String>,
    },
    /// A config file could not be read.
    Config {
        file: Option<String>,
        culprit: Option<Culprit>,
    },
    /// The crash happened inside a mod.
    Mod {
        culprit: Culprit,
        error: String,
    },
    /// The world crashes on one entity or block, and no mod in the stack tells why.
    Ticking(Ticking),
}

impl Cause {
    /// Whether the loader refused to start the game, which may end in a clean exit.
    fn refusal(&self) -> bool {
        matches!(self, Self::Dependencies(_) | Self::Duplicates(_))
    }

    /// The single mod to update, disable or remove, when the cause names one.
    pub fn culprit(&self) -> Option<&Culprit> {
        match self {
            Self::Mismatch { culprit, .. }
            | Self::Mixin(culprit)
            | Self::Load { culprit, .. }
            | Self::Mod { culprit, .. } => Some(culprit),
            _ => None,
        }
    }
}

/// What most likely made a game stop, and the file that tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub cause: Cause,
    /// The report or log the cause was found in; `None` when only the game's output told.
    pub source: Option<PathBuf>,
    /// What the game was updating, when the crash report says and the cause is not that already.
    pub ticking: Option<Ticking>,
}

/// Looks for why the game in `game_dir` stopped: crash reports written since `since`, the log and the game's `output`.
/// With `crashed` false only a refusal to load is reported, since a clean exit has nothing else to explain.
pub fn diagnose(
    game_dir: &Path,
    since: SystemTime,
    crashed: bool,
    output: &[String],
) -> Option<Diagnosis> {
    let fresh: Vec<logs::LogFile> = logs::list(game_dir)
        .into_iter()
        .filter(|f| f.kind == logs::LogKind::Crash && f.modified >= since)
        .collect();
    let report = fresh.iter().find(|f| !is_jvm(&f.path));
    let jvm = fresh.iter().find(|f| is_jvm(&f.path));
    let crashed = crashed || report.is_some() || jvm.is_some();
    let log = game_dir.join("logs").join("latest.log");
    let fresh_log = std::fs::metadata(&log)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m >= since);
    let mut texts: Vec<(TextKind, String, Option<PathBuf>)> = [
        report.map(|f| (TextKind::Report, f.path.clone())),
        jvm.map(|f| (TextKind::Jvm, f.path.clone())),
        fresh_log.then_some((TextKind::Log, log)),
    ]
    .into_iter()
    .flatten()
    .filter_map(|(kind, path)| Some((kind, logs::read(&path).ok()?, Some(path))))
    .collect();
    if !output.is_empty() {
        texts.push((TextKind::Output, output.join("\n"), None));
    }
    if texts.is_empty() {
        return None;
    }
    let index = ModIndex::scan(&game_dir.join("mods"));
    let refs: Vec<(TextKind, &str)> = texts.iter().map(|(k, t, _)| (*k, t.as_str())).collect();
    let (cause, at) = analyze(&refs, &index)?;
    if !crashed && !cause.refusal() {
        return None;
    }
    let ticking = texts
        .iter()
        .find(|(k, ..)| *k == TextKind::Report)
        .and_then(|(_, t, _)| ticking(t));
    Some(diagnosis(cause, texts.swap_remove(at).2, ticking))
}

/// Explains one crash report or `hs_err` file, as the Logs tab shows it.
pub fn explain(game_dir: &Path, path: &Path) -> Option<Diagnosis> {
    let text = logs::read(path).ok()?;
    let kind = if is_jvm(path) {
        TextKind::Jvm
    } else {
        TextKind::Report
    };
    let index = ModIndex::scan(&game_dir.join("mods"));
    let (cause, _) = analyze(&[(kind, &text)], &index)?;
    Some(diagnosis(cause, Some(path.to_owned()), ticking(&text)))
}

fn diagnosis(cause: Cause, source: Option<PathBuf>, ticking: Option<Ticking>) -> Diagnosis {
    let ticking = ticking.filter(|_| !matches!(cause, Cause::Ticking(_)));
    Diagnosis {
        cause,
        source,
        ticking,
    }
}

/// Whether a file is a JVM crash log rather than the game's crash report.
pub fn is_jvm(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("hs_err_pid"))
}

/// Mod ids, names and files of an instance, and which mod each class package belongs to.
#[derive(Debug, Default)]
pub struct ModIndex {
    mods: HashMap<String, (Option<String>, PathBuf, bool)>,
    packages: HashMap<String, String>,
}

impl ModIndex {
    /// Reads the enabled jars in `dir`: their metadata and class listing, not their contents.
    pub fn scan(dir: &Path) -> Self {
        let mut index = Self::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return index;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jar") {
                continue;
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
                continue;
            };
            let Ok(meta) = riven_resolve::JarMeta::read_zip(&mut zip) else {
                continue;
            };
            let Some(main) = meta.mods.first() else {
                continue;
            };
            let mut jars = vec![(&meta, false)];
            while let Some((jar, nested)) = jars.pop() {
                for m in &jar.mods {
                    index
                        .mods
                        .entry(m.id.clone())
                        .or_insert_with(|| (m.name.clone(), path.clone(), nested));
                }
                jars.extend(jar.nested.iter().map(|j| (j, true)));
            }
            for name in zip.file_names().flatten() {
                let Some((package, _)) = name.rsplit_once('/') else {
                    continue;
                };
                let package = package.replace('/', ".");
                // Some mods carry classes in the game's packages; those frames stay the game's.
                if name.ends_with(".class")
                    && !name.starts_with("META-INF/")
                    && !CORE_PACKAGES
                        .iter()
                        .any(|p| format!("{package}.").starts_with(p))
                {
                    index
                        .packages
                        .entry(package)
                        .or_insert_with(|| main.id.clone());
                }
            }
        }
        index
    }

    fn culprit(&self, id: &str) -> Culprit {
        let mut culprit = Culprit::new(id);
        if let Some((name, file, nested)) = self.mods.get(id) {
            culprit.name = name.clone();
            culprit.file = Some(file.clone());
            culprit.nested = *nested;
        }
        culprit
    }

    fn owner(&self, class: &str) -> Option<Culprit> {
        let (package, _) = class.rsplit_once('.')?;
        self.packages.get(package).map(|id| self.culprit(id))
    }
}

type Rule = fn(&str, &ModIndex) -> Option<Cause>;

/// Finds the most likely cause and the index of the text that tells it.
/// Rules go from the surest to the vaguest; each looks only at the kinds of text it can trust.
pub fn analyze(texts: &[(TextKind, &str)], index: &ModIndex) -> Option<(Cause, usize)> {
    use TextKind::{Jvm, Log, Output, Report};
    let rules: &[(&[TextKind], Rule)] = &[
        (&[Report, Log], dependencies),
        (&[Report, Log], duplicates),
        (&[Report, Log, Output], overlap),
        (&[Report, Log, Output], java),
        (&[Jvm, Output], memory_settings),
        (&[Report, Log, Output], out_of_memory),
        (&[Jvm, Output], native),
        (&[Report, Log, Output, Jvm], disk_full),
        (&[Report, Log], mixin),
        (&[Report, Log], load_failure),
        (&[Report, Output], corrupt),
        (&[Report], config),
        (&[Report, Output], graphics),
        (&[Report], stack),
        (&[Report], |text, _| ticking(text).map(Cause::Ticking)),
    ];
    rules.iter().find_map(|(kinds, rule)| {
        texts
            .iter()
            .enumerate()
            .filter(|(_, (kind, _))| kinds.contains(kind))
            .find_map(|(i, (_, text))| rule(text, index).map(|c| (c, i)))
    })
}

/// Unmet requirements in NeoForge's and Fabric's words.
fn dependencies(text: &str, index: &ModIndex) -> Option<Cause> {
    let mut out = Vec::new();
    let mut neoforge = false;
    for line in text.lines() {
        if line.contains("Missing or unsupported mandatory dependencies:")
            || line.contains("Unsupported installed optional dependencies:")
        {
            neoforge = true;
            continue;
        }
        let line = line.trim();
        if neoforge && let Some(dep) = neoforge_dependency(line, index) {
            if !out.contains(&dep) {
                out.push(dep);
            }
            continue;
        }
        neoforge = false;
        if let Some(dep) = line
            .strip_prefix("- ")
            .and_then(|l| fabric_dependency(l, index))
            && !out.contains(&dep)
        {
            out.push(dep);
        }
    }
    (!out.is_empty()).then_some(Cause::Dependencies(out))
}

/// `Mod ID: 'create', Requested by: 'createaddition', Expected range: '[6.0.0,)', Actual version: '[MISSING]'`.
fn neoforge_dependency(line: &str, index: &ModIndex) -> Option<Dependency> {
    let found = quoted(line, "Actual version")?;
    Some(Dependency {
        by: index.culprit(quoted(line, "Requested by")?),
        needs: index.culprit(quoted(line, "Mod ID")?),
        range: quoted(line, "Expected range")?.to_owned(),
        found: (found != "[MISSING]").then(|| found.to_owned()),
        incompatible: false,
    })
}

/// The value of `name: 'value'` in a line.
fn quoted<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(&format!("{name}: '"))? + name.len() + 3;
    let len = line[start..].find('\'')?;
    Some(&line[start..start + len])
}

/// `Mod 'Name' (id) 1.0 requires version 2.0 or later of mod 'Other' (other), but only the wrong version is present: 1.5!`
/// or `… requires any version of other, which is missing!`; `is incompatible with` for breaks.
fn fabric_dependency(line: &str, index: &ModIndex) -> Option<Dependency> {
    let rest = line.strip_prefix("Mod '")?;
    let (by_name, rest) = rest.split_once("' (")?;
    let (by_id, rest) = rest.split_once(") ")?;
    let (_, rest) = rest.split_once(' ')?;
    let (incompatible, rest) = if let Some(r) = rest.strip_prefix("requires ") {
        (false, r)
    } else {
        (true, rest.strip_prefix("is incompatible with ")?)
    };
    let (range, rest) = rest.split_once(" of ")?;
    let (target, outcome) = rest.split_once(", ")?;
    let (needs_name, needs_id) = match target.strip_prefix("mod '") {
        Some(t) => {
            let (name, id) = t.split_once("' (")?;
            (Some(name), id.strip_suffix(')')?)
        }
        None => (None, target),
    };
    let found = outcome
        .rsplit_once("present: ")
        .map(|(_, v)| v.trim_end_matches('!').to_owned());
    if !incompatible && found.is_none() && !outcome.starts_with("which is missing") {
        return None;
    }
    let mut needs = index.culprit(needs_id);
    if let Some(name) = needs_name {
        needs = needs.named(name);
    }
    Some(Dependency {
        by: index.culprit(by_id).named(by_name),
        needs,
        range: range.to_owned(),
        found,
        incompatible,
    })
}

/// (Neo)Forge's `Mod ID: 'jei' from mod files: a.jar, b.jar` in the log and `Mod jei is present in multiple files: a.jar, b.jar`.
fn duplicates(text: &str, index: &ModIndex) -> Option<Cause> {
    let mut out: Vec<Duplicate> = Vec::new();
    for line in text.lines() {
        let line = strip_formatting(line.trim());
        let found = if let Some(files) =
            line.split_once("' from mod files: ")
                .and_then(|(head, files)| {
                    Some((quoted(&format!("{head}'"), "Mod ID")?.to_owned(), files))
                }) {
            Some(files)
        } else {
            line.split_once(" is present in multiple files: ")
                .and_then(|(head, files)| Some((head.rsplit_once("Mod ")?.1.to_owned(), files)))
        };
        let Some((id, files)) = found else {
            continue;
        };
        if out.iter().any(|d| d.culprit.id == id) {
            continue;
        }
        out.push(Duplicate {
            culprit: index.culprit(&id),
            files: files
                .split(',')
                .map(|f| f.trim().to_owned())
                .filter(|f| !f.is_empty())
                .collect(),
        });
    }
    (!out.is_empty()).then_some(Cause::Duplicates(out))
}

/// Drops Minecraft's `§x` colour codes.
fn strip_formatting(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Java's module system: `Modules create and flywheel export package com.x to module y`.
fn overlap(text: &str, index: &ModIndex) -> Option<Cause> {
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("Modules ")?;
        let (first, rest) = rest.split_once(" and ")?;
        let (second, rest) = rest.split_once(" export package ")?;
        let package = rest.split_whitespace().next()?;
        Some(Cause::Overlap {
            first: index.culprit(first),
            second: index.culprit(second),
            package: package.to_owned(),
        })
    })
}

/// A Java older or newer than the game needs.
fn java(text: &str, _: &ModIndex) -> Option<Cause> {
    if let Some(needed) = java_needed(text) {
        return Some(Cause::Java { needed });
    }
    // Forge before 1.17 casts the class loader to the one Java 8 has.
    text.contains("cannot be cast to class java.net.URLClassLoader")
        .then_some(Cause::JavaTooNew)
}

/// `UnsupportedClassVersionError: … class file version 65.0` means Java 21 (class file versions start at 45 for Java 1).
fn java_needed(text: &str) -> Option<u32> {
    let line = text
        .lines()
        .find(|l| l.contains("UnsupportedClassVersionError"))?;
    let (_, rest) = line.split_once("class file version ")?;
    let major: u32 = rest.split(['.', ' ', ')']).next()?.parse().ok()?;
    (major > 44).then(|| major - 44)
}

/// What Java prints when it cannot get the memory it is told to use.
fn memory_settings(text: &str, _: &ModIndex) -> Option<Cause> {
    if text.contains("There is insufficient memory for the Java Runtime Environment")
        || text.contains("Native memory allocation (mmap) failed")
    {
        return Some(Cause::SystemMemory);
    }
    [
        "Could not reserve enough space for",
        "Invalid maximum heap size",
        "Invalid initial heap size",
        "Initial heap size set to a larger value than the maximum heap size",
        "Too small maximum heap",
    ]
    .iter()
    .any(|m| text.contains(m))
    .then_some(Cause::HeapSize)
}

fn out_of_memory(text: &str, _: &ModIndex) -> Option<Cause> {
    exceptions(text)
        .iter()
        .any(|e| e.header.starts_with("java.lang.OutOfMemoryError"))
        .then_some(Cause::Memory)
}

/// The library under `# Problematic frame:` in a JVM crash: `# C  [nvoglv64.dll+0xabc]`.
fn native(text: &str, _: &ModIndex) -> Option<Cause> {
    let mut lines = text.lines();
    lines.find(|l| l.contains("# Problematic frame:"))?;
    let frame = lines.next()?.trim_start_matches('#').trim();
    Some(Cause::Native(native_part(frame)))
}

fn native_part(frame: &str) -> Native {
    let library = frame
        .split_once('[')
        .and_then(|(_, rest)| rest.split(['+', ']']).next())
        .map(str::trim)
        .filter(|l| !l.is_empty());
    let compiled = matches!(frame.chars().next(), Some('V' | 'J' | 'j' | 'A'));
    let Some(library) = library.filter(|_| !compiled) else {
        return Native::Java;
    };
    let lower = library.to_lowercase();
    if lower.starts_with("libjvm") || lower.starts_with("jvm.") {
        return Native::Java;
    }
    if let Some((vendor, _)) = DRIVERS
        .iter()
        .find(|(_, names)| names.iter().any(|n| lower.contains(n)))
    {
        return Native::Driver(Some(*vendor));
    }
    if GRAPHICS_LIBRARIES.iter().any(|n| lower.contains(n)) {
        return Native::Driver(None);
    }
    Native::Library(library.to_owned())
}

fn disk_full(text: &str, _: &ModIndex) -> Option<Cause> {
    DISK_FULL
        .iter()
        .any(|m| text.contains(m))
        .then_some(Cause::DiskFull)
}

fn mixin(text: &str, index: &ModIndex) -> Option<Cause> {
    mixin_culprit(text).map(|id| Cause::Mixin(index.culprit(&id)))
}

/// The mod named by a mixin failure: NeoForge's message, Mixin's own `from mod x`, Fabric's `Mixin apply for mod x failed`.
fn mixin_culprit(text: &str) -> Option<String> {
    // Optional mixins that fail are logged as warnings, trace included, and the game goes on.
    let mut survived = false;
    for line in text.lines() {
        if line.starts_with('[') {
            survived = ["/WARN]", "/INFO]", "/DEBUG]", "/TRACE]"]
                .iter()
                .any(|m| line.contains(m));
        }
        if survived || line.trim_start().starts_with("at ") {
            continue;
        }
        if let Some((_, rest)) = line.split_once("Mixin apply for mod ") {
            return rest.split_whitespace().next().map(str::to_owned);
        }
        if line.contains("Mixin application of ")
            && let Some((_, rest)) = line.split_once(" (")
        {
            return rest.split(')').next().map(str::to_owned);
        }
        if (line.contains("MixinApplyError")
            || line.contains("InjectionError")
            || line.contains("MixinTransformerError")
            || line.contains("InvalidMixinException"))
            && let Some((_, rest)) = line.split_once(" from mod ")
        {
            return rest
                .split(|c: char| c.is_whitespace() || c == ']' || c == ',')
                .next()
                .map(str::to_owned);
        }
    }
    None
}

/// A mod the loader names as failing to start: (Neo)Forge's crash report sections and log lines, Fabric's entrypoints.
fn load_failure(text: &str, index: &ModIndex) -> Option<Cause> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let after = &lines[i + 1..];
        if let Some(id) = line
            .strip_prefix("-- Mod loading issue for: ")
            .or_else(|| line.strip_prefix("-- MOD "))
            .and_then(|l| l.strip_suffix(" --"))
            && let Some(cause) = load_issue(id, after, index)
        {
            return Some(cause);
        }
        let id = if let Some((_, rest)) = line.split_once("Failed to create mod instance. ModID: ")
        {
            rest.split(',').next()
        } else if line.contains("Could not execute entrypoint stage") {
            line.split_once("provided by '")
                .and_then(|(_, rest)| rest.split('\'').next())
        } else if let Some((_, rest)) =
            line.split_once("LoaderExceptionModCrash: Caught exception from ")
        {
            rest.rsplit_once(" (")
                .and_then(|(_, id)| id.split(')').next())
        } else {
            None
        };
        if let Some(id) = id {
            return Some(Cause::Load {
                culprit: index.culprit(id.trim()),
                error: root_error(after).map(short_error).unwrap_or_default(),
            });
        }
    }
    None
}

/// One `-- Mod loading issue for: id --` (NeoForge) or `-- MOD id --` (Forge) section of a crash report.
fn load_issue(id: &str, details: &[&str], index: &ModIndex) -> Option<Cause> {
    let details: Vec<&str> = details
        .iter()
        .take_while(|l| !l.starts_with("-- "))
        .map(|l| l.trim())
        .collect();
    let failure = details
        .iter()
        .find_map(|l| l.strip_prefix("Failure message: "))?;
    if ![
        "has failed to load correctly",
        "has class loading errors",
        "encountered an error processing deferred work",
    ]
    .iter()
    .any(|m| failure.contains(m))
    {
        return None;
    }
    let deepest = details.iter().rev().find_map(|l| {
        l.strip_prefix("Caused by ")?
            .split_once(": ")
            .map(|(_, e)| e)
    });
    let message = details
        .iter()
        .find_map(|l| l.strip_prefix("Exception message: "))
        .filter(|m| !m.starts_with('<'));
    let mut culprit = index.culprit(id);
    if let Some((name, _)) = failure.split_once(" (") {
        culprit = culprit.named(name);
    }
    Some(Cause::Load {
        culprit,
        error: deepest.or(message).map(short_error).unwrap_or_default(),
    })
}

/// The deepest cause of the stack trace that starts among `lines`.
fn root_error<'a>(lines: &[&'a str]) -> Option<&'a str> {
    let mut last = None;
    for line in lines.iter().take(200) {
        let trimmed = line.trim_start();
        if is_exception_header(trimmed) {
            last = Some(trimmed.strip_prefix("Caused by: ").unwrap_or(trimmed));
        } else if last.is_some() && !trimmed.starts_with("at ") && !trimmed.starts_with("...") {
            break;
        }
    }
    last
}

/// A jar cut short: `zip END header not found`, NeoForge's `File x.jar is not a jar file`.
fn corrupt(text: &str, _: &ModIndex) -> Option<Cause> {
    let line = text
        .lines()
        .find(|l| CORRUPT.iter().any(|m| l.contains(m)))?;
    let file = line
        .split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '[' | ']' | '(' | ')'))
        .find(|w| w.ends_with(".jar"))
        .map(|w| w.rsplit(['/', '\\']).next().unwrap_or(w).to_owned());
    Some(Cause::Corrupt { file })
}

/// Forge's `Failed loading config file create-client.toml of type CLIENT for modid create`, or a NightConfig parse error.
fn config(text: &str, index: &ModIndex) -> Option<Cause> {
    if let Some(rest) = text
        .lines()
        .find_map(|l| l.split_once("Failed loading config file ").map(|(_, r)| r))
    {
        let file = rest.split(" of type ").next().map(str::to_owned);
        let culprit = rest
            .split_once(" for modid ")
            .and_then(|(_, id)| id.split_whitespace().next())
            .map(|id| index.culprit(id));
        return Some(Cause::Config { file, culprit });
    }
    let errors = exceptions(text);
    errors
        .iter()
        .any(|e| {
            e.header
                .starts_with("com.electronwill.nightconfig.core.io.ParsingException")
        })
        .then(|| Cause::Config {
            file: None,
            culprit: errors
                .iter()
                .find_map(|e| e.frames.iter().find_map(|f| frame_mod(f, index))),
        })
}

fn graphics(text: &str, _: &ModIndex) -> Option<Cause> {
    GRAPHICS
        .iter()
        .any(|m| text.contains(m))
        .then_some(Cause::Graphics)
}

/// The mod in whose code the crash happened, from the report's stack trace.
fn stack(text: &str, index: &ModIndex) -> Option<Cause> {
    let errors = exceptions(text);
    for error in errors.iter().rev() {
        let Some(culprit) = error.frames.iter().find_map(|f| frame_mod(f, index)) else {
            continue;
        };
        if let Some(class) = missing_class(error.header) {
            let other = if CORE_PACKAGES.iter().any(|p| class.starts_with(p)) {
                None
            } else {
                index.owner(&class).filter(|o| o.id != culprit.id)
            };
            return Some(Cause::Mismatch { culprit, other });
        }
        return Some(Cause::Mod {
            culprit,
            error: short_error(error.header),
        });
    }
    errors
        .iter()
        .rev()
        .find_map(|e| missing_class(e.header))
        .map(|class| Cause::MissingClass { class })
}

struct Exception<'a> {
    header: &'a str,
    frames: Vec<&'a str>,
}

/// The exceptions and their causes as they appear: a header line, then its `at` frames.
/// Any other line ends a trace, so a thread dump's frames are not taken for the exception's.
fn exceptions(text: &str) -> Vec<Exception<'_>> {
    let mut out: Vec<Exception> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(frame) = trimmed.strip_prefix("at ") {
            if open && let Some(last) = out.last_mut() {
                last.frames.push(frame);
            }
        } else if is_exception_header(trimmed) {
            out.push(Exception {
                header: trimmed.strip_prefix("Caused by: ").unwrap_or(trimmed),
                frames: Vec::new(),
            });
            open = true;
        } else if !trimmed.starts_with("...") {
            open = false;
        }
    }
    out
}

fn is_exception_header(line: &str) -> bool {
    let line = line.strip_prefix("Caused by: ").unwrap_or(line);
    let name = line.split([':', ' ']).next().unwrap_or("");
    name.contains('.')
        && !name.contains('/')
        && (name.ends_with("Exception") || name.ends_with("Error") || name.ends_with("Throwable"))
}

/// The mod a frame runs in: `TRANSFORMER/ponder@1.0.82/…` on (Neo)Forge, the class's package elsewhere.
fn frame_mod(frame: &str, index: &ModIndex) -> Option<Culprit> {
    let location = frame.split('(').next()?;
    let mut parts = location.split('/');
    if let (Some(_), Some(module), Some(_)) = (parts.next(), parts.next(), parts.next())
        && let Some((id, _)) = module.split_once('@')
    {
        // Java modules carry versions too (`com.google.common@32.1`); mod ids have no dots.
        return (!CORE.contains(&id) && !id.contains('.')).then(|| index.culprit(id));
    }
    let class = location.rsplit_once('.')?.0;
    let class = class.rsplit('/').next()?;
    index.owner(class)
}

/// The class a linkage error says is missing or changed.
fn missing_class(header: &str) -> Option<String> {
    let (name, message) = header.split_once(": ")?;
    let kind = name.rsplit('.').next()?;
    match kind {
        "NoSuchMethodError" => {
            let method = message.split_whitespace().find(|w| w.contains('('))?;
            let method = method.trim_start_matches('\'').split('(').next()?;
            Some(method.rsplit_once('.')?.0.to_owned())
        }
        "NoSuchFieldError" => message
            .strip_prefix("Class ")?
            .split_whitespace()
            .next()
            .map(str::to_owned),
        "NoClassDefFoundError" | "ClassNotFoundException" => {
            Some(message.split_whitespace().next()?.replace('/', "."))
        }
        _ => None,
    }
}

/// The entity or block a crash report's `-- Entity being ticked --` or `-- Block entity being ticked --` section names.
fn ticking(text: &str) -> Option<Ticking> {
    let mut kind = None;
    let (mut id, mut at) = (None, None);
    for line in text.lines() {
        if let Some(section) = line.strip_prefix("-- ").and_then(|l| l.strip_suffix(" --")) {
            if id.is_some() {
                break;
            }
            kind = match section {
                "Entity being ticked" => Some(TickingKind::Entity),
                "Block entity being ticked" | "Block being ticked" => Some(TickingKind::Block),
                _ => None,
            };
            continue;
        }
        let line = line.trim();
        match kind {
            Some(TickingKind::Entity) => {
                if let Some(v) = line.strip_prefix("Entity Type: ") {
                    id = v.split(" (").next().map(str::to_owned);
                } else if let Some(v) = line.strip_prefix("Entity's Exact location: ") {
                    at = coordinates(v);
                }
            }
            Some(TickingKind::Block) => {
                if let Some(v) = line.strip_prefix("Name: ") {
                    id = v.split(" // ").next().map(str::to_owned);
                } else if let Some(v) = line.strip_prefix("Block: Block{") {
                    id = id.or_else(|| v.split('}').next().map(str::to_owned));
                } else if let Some(v) = line.strip_prefix("Block location: World: (") {
                    at = v.split(')').next().and_then(coordinates);
                }
            }
            None => {}
        }
    }
    Some(Ticking {
        kind: kind?,
        id: id.filter(|id| id != "minecraft:player")?,
        at,
    })
}

/// `12.50, 64.00, -3.20` or `10,64,-5` as block coordinates `12, 64, -4`.
fn coordinates(text: &str) -> Option<String> {
    let parts: Vec<i64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>().map(|v| v.floor() as i64))
        .collect::<Result<_, _>>()
        .ok()?;
    (parts.len() == 3).then(|| format!("{}, {}, {}", parts[0], parts[1], parts[2]))
}

/// `java.lang.NullPointerException: long message` → `NullPointerException: long message`, cut to one short line.
fn short_error(header: &str) -> String {
    let (name, message) = header.split_once(": ").unwrap_or((header, ""));
    let name = name.rsplit('.').next().unwrap_or(name);
    let mut out = if message.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {message}")
    };
    if out.chars().count() > 200 {
        out = out.chars().take(199).collect::<String>() + "…";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/crash")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    fn index(mods: &[(&str, &str, &[&str])]) -> ModIndex {
        let mut index = ModIndex::default();
        for (id, name, packages) in mods {
            index.mods.insert(
                (*id).to_owned(),
                (
                    Some((*name).to_owned()),
                    PathBuf::from(format!("{id}.jar")),
                    false,
                ),
            );
            for p in *packages {
                index.packages.insert((*p).to_owned(), (*id).to_owned());
            }
        }
        index
    }

    fn one(kind: TextKind, text: &str, index: &ModIndex) -> Option<Cause> {
        analyze(&[(kind, text)], index).map(|(c, _)| c)
    }

    fn report(text: &str) -> Option<Cause> {
        one(TextKind::Report, text, &ModIndex::default())
    }

    #[test]
    fn a_crash_inside_a_mod_names_it_not_the_game_around_it() {
        let index = index(&[("ponder", "Ponder", &[])]);
        let Some(Cause::Mod { culprit, error }) =
            one(TextKind::Report, &fixture("neoforge-npe.txt"), &index)
        else {
            panic!("no mod found");
        };
        assert_eq!(culprit.label(), "Ponder");
        assert!(error.starts_with("NullPointerException: Cannot invoke"));
    }

    #[test]
    fn a_missing_method_blames_the_caller_and_names_the_owner() {
        let index = index(&[
            ("stockmarket", "Create: Stock Market", &[]),
            (
                "numismatics",
                "Create: Numismatics",
                &["dev.ithundxr.createnumismatics.content.vendor"],
            ),
        ]);
        let cause = one(
            TextKind::Report,
            &fixture("neoforge-nosuchmethod.txt"),
            &index,
        );
        let Some(Cause::Mismatch { culprit, other }) = cause else {
            panic!("{cause:?}");
        };
        assert_eq!(culprit.id, "stockmarket");
        assert_eq!(other.map(|o| o.id).as_deref(), Some("numismatics"));
    }

    #[test]
    fn a_missing_game_class_means_another_game_version() {
        let text = "java.lang.NoSuchMethodError: 'void net.minecraft.world.entity.Entity.foo()'\n\
                    \tat TRANSFORMER/oldmod@1.0/com.old.Mod.run(Mod.java:1) ~[oldmod.jar:1.0]\n";
        assert!(matches!(
            report(text),
            Some(Cause::Mismatch { other: None, .. })
        ));
    }

    #[test]
    fn a_missing_class_with_no_mod_in_the_stack_is_still_named() {
        let text = "java.lang.NoClassDefFoundError: dev/architectury/event/Event\n\
                    \tat java.base/java.lang.Class.forName0(Native Method)\n";
        assert_eq!(
            report(text),
            Some(Cause::MissingClass {
                class: "dev.architectury.event.Event".into()
            })
        );
    }

    #[test]
    fn fabric_frames_are_matched_by_package() {
        let text = "java.lang.IllegalStateException: boom\n\
                    \tat net.minecraft.class_310.method_1514(class_310.java:10)\n\
                    \tat me.jellysquid.mods.sodium.client.Render.draw(Render.java:5)\n";
        let index = index(&[("sodium", "Sodium", &["me.jellysquid.mods.sodium.client"])]);
        let Some(Cause::Mod { culprit, .. }) = one(TextKind::Report, text, &index) else {
            panic!();
        };
        assert_eq!(culprit.id, "sodium");
    }

    #[test]
    fn neoforge_dependency_errors_are_read_from_the_log() {
        let log = "[12:00:00] [main/ERROR] [net.neoforged.fml.loading.ModSorter/LOADING]: Missing or unsupported mandatory dependencies:\n\
                   \tMod ID: 'create', Requested by: 'createaddition', Expected range: '[6.0.0,)', Actual version: '[MISSING]'\n\
                   \tMod ID: 'neoforge', Requested by: 'jei', Expected range: '[21.1.200,)', Actual version: '21.1.100'\n\
                   [12:00:01] [main/INFO] [other]: done\n";
        let index = index(&[("createaddition", "Create Crafts & Additions", &[])]);
        let Some(Cause::Dependencies(deps)) = one(TextKind::Log, log, &index) else {
            panic!();
        };
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].by.label(), "Create Crafts & Additions");
        assert_eq!(deps[0].needs.id, "create");
        assert_eq!(deps[0].found, None);
        assert_eq!(deps[1].found.as_deref(), Some("21.1.100"));
    }

    #[test]
    fn fabric_dependency_errors_keep_the_loaders_wording() {
        let log = "[main/ERROR]: Incompatible mods found!\n\
                   net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible with the game or each other!\n\
                   A potential solution has been determined, this may resolve your problem:\n\
                   \t - Install fabric-api, any version.\n\
                   More details:\n\
                   \t - Mod 'Sodium Extra' (sodium-extra) 0.5.4 requires any version of fabric-api, which is missing!\n\
                   \t - Mod 'Iris' (iris) 1.7.0 requires version 0.5.0 or later of mod 'Sodium' (sodium), but only the wrong version is present: 0.4.10!\n\
                   \t - Mod 'Mod Menu' (modmenu) 9.0 recommends any version of fabric-api, which is missing!\n\
                   \t - Mod 'Indium' (indium) 1.0 is incompatible with any version of mod 'Sodium' (sodium), yet a conflicting version is present: 0.4.10!\n";
        let Some(Cause::Dependencies(deps)) = one(TextKind::Log, log, &ModIndex::default()) else {
            panic!();
        };
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].by.label(), "Sodium Extra");
        assert_eq!(deps[0].needs.id, "fabric-api");
        assert_eq!(deps[0].range, "any version");
        assert_eq!(deps[0].found, None);
        assert_eq!(deps[1].needs.label(), "Sodium");
        assert_eq!(deps[1].range, "version 0.5.0 or later");
        assert_eq!(deps[1].found.as_deref(), Some("0.4.10"));
        assert!(deps[2].incompatible);
    }

    #[test]
    fn java_memory_and_mixins_are_told_apart() {
        let java = "java.lang.UnsupportedClassVersionError: net/minecraft/client/main/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0";
        assert_eq!(report(java), Some(Cause::Java { needed: 21 }));

        let memory = "Description: Unexpected error\n\njava.lang.OutOfMemoryError: Java heap space\n\tat java.base/java.util.Arrays.copyOf(Arrays.java:1)\n";
        assert_eq!(report(memory), Some(Cause::Memory));

        let mixin = "org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered\n\
                     Caused by: org.spongepowered.asm.mixin.injection.throwables.InjectionError: Critical injection failure: @Inject annotation on render could not find any targets matching 'draw' in net.minecraft.class_757. Using refmap iris-refmap.json [PREINJECT Applicator Phase -> iris.mixins.json:MixinGameRenderer from mod iris -> Prepare Injections]\n";
        let Some(Cause::Mixin(culprit)) = report(mixin) else {
            panic!();
        };
        assert_eq!(culprit.id, "iris");
    }

    #[test]
    fn exceptions_a_log_survived_are_not_causes() {
        let log = "[12:00:00] [Render thread/ERROR] [mezz.jei.library.load.PluginCaller/]: Caught an error from mod plugin\n\
                   java.lang.IllegalArgumentException: Tried to register a recipe type twice\n\
                   \tat TRANSFORMER/jei@19.39.0/mezz.jei.library.recipes.RecipeTypeData.register(RecipeTypeData.java:20) ~[jei.jar:?]\n\
                   [12:00:01] [Render thread/INFO]: OpenGL debug message: id=1282, source=API, type=ERROR, severity=HIGH, message='GL_INVALID_OPERATION error generated. No active program.'\n\
                   [12:00:02] [Worker/ERROR]: Failed to load pack: java.util.zip.ZipException: zip END header not found\n";
        let index = index(&[("jei", "Just Enough Items", &[])]);
        assert_eq!(one(TextKind::Log, log, &index), None);
        assert!(matches!(
            one(TextKind::Report, log, &index),
            Some(Cause::Corrupt { .. })
        ));
    }

    #[test]
    fn a_mixin_failure_logged_as_a_warning_does_not_outrank_the_report() {
        let log = "[18Sep2026 21:20:43.793] [Worker-ResourceReload-6/WARN] [mixin/]: Mixin apply for mod alltheleaks failed alltheleaks.mixins.json:main.AthenaResourceLoaderMixin from mod alltheleaks -> earth.terrarium.athena.impl.loading.AthenaResourceLoader: org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException STATIC modifier of @Shadow method setGetter\n\
                   org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException: STATIC modifier of @Shadow method setGetter in alltheleaks.mixins.json:main.AthenaResourceLoaderMixin from mod alltheleaks does not match the target\n";
        let report = fixture("neoforge-npe.txt");
        let index = index(&[("ponder", "Ponder", &[])]);
        let found = analyze(&[(TextKind::Report, &report), (TextKind::Log, log)], &index);
        assert!(matches!(found, Some((Cause::Mod { .. }, 0))), "{found:?}");
    }

    #[test]
    fn java_modules_are_not_mods() {
        let text = "java.lang.IllegalStateException: boom\n\
                    \tat TRANSFORMER/com.google.common@32.1.2-jre/com.google.common.base.Preconditions.checkState(Preconditions.java:1)\n";
        assert_eq!(report(text), None);
    }

    #[test]
    fn debug_log_mentions_and_applied_mixins_are_not_causes() {
        let noise = "[main/TRACE] [mixin/]: Added class metadata for java/lang/OutOfMemoryError to metadata cache\n\
                     \tat TRANSFORMER/minecraft@1.21.1/net.minecraft.Foo.bar(Foo.java) {re:mixin,pl:mixin:APP:lootjs.mixins.json:Mixin from mod lootjs}\n";
        assert_eq!(one(TextKind::Log, noise, &ModIndex::default()), None);
    }

    #[test]
    fn a_watchdog_thread_dump_does_not_blame_whatever_thread_ran_a_mod() {
        let text = "java.lang.Error: Watchdog (Client shutdown from post-main)\n\
                    \tat knot//net.minecraft.server.dedicated.ServerWatchdog.createWatchdogCrashReport(ServerWatchdog.java:83)\n\
                    \n\
                    -- Thread Dump --\n\
                    \"Worker-1\" daemon prio=5 Id=40 RUNNABLE\n\
                    \tat me.jellysquid.mods.sodium.client.Render.draw(Render.java:5)\n";
        let index = index(&[("sodium", "Sodium", &["me.jellysquid.mods.sodium.client"])]);
        assert_eq!(one(TextKind::Report, text, &index), None);
    }

    #[test]
    fn neoforge_load_failures_name_the_mod_and_its_deepest_cause() {
        let text = "Description: Mod loading failures have occurred; consult the issue messages for more details\n\n\
                    -- Mod loading issue for: sable --\n\
                    Details:\n\
                    \tCaused by 0: java.lang.reflect.InvocationTargetException\n\
                    \tCaused by 1: java.lang.IllegalStateException: Registry is already frozen\n\
                    \tMod file: /game/mods/sable-1.0.jar\n\
                    \tFailure message: Sable (sable) has failed to load correctly\n\
                    \t\tjava.lang.reflect.InvocationTargetException: null\n\
                    \tMod version: 1.0\n\
                    \tException message: java.lang.reflect.InvocationTargetException\n";
        let Some(Cause::Load { culprit, error }) = report(text) else {
            panic!();
        };
        assert_eq!(culprit.label(), "Sable");
        assert_eq!(error, "IllegalStateException: Registry is already frozen");
    }

    #[test]
    fn fabric_entrypoint_failures_name_the_mod_from_the_log() {
        let log = "[main/ERROR]: Minecraft has crashed!\n\
                   net.fabricmc.loader.impl.FormattedException: java.lang.RuntimeException: Could not execute entrypoint stage 'main' due to errors, provided by 'lithostitched' at 'dev.worldgen.lithostitched.Lithostitched'!\n\
                   \tat net.fabricmc.loader.impl.FabricLoaderImpl.invokeEntrypoints(FabricLoaderImpl.java:1)\n\
                   Caused by: java.lang.NullPointerException: config\n\
                   \tat dev.worldgen.lithostitched.Lithostitched.onInitialize(Lithostitched.java:3)\n";
        let Some(Cause::Load { culprit, error }) = one(TextKind::Log, log, &ModIndex::default())
        else {
            panic!();
        };
        assert_eq!(culprit.id, "lithostitched");
        assert_eq!(error, "NullPointerException: config");
    }

    #[test]
    fn duplicates_are_read_in_both_loader_wordings() {
        let log = "[main/ERROR] [net.neoforged.fml.loading.UniqueModListBuilder/LOADING]: Found duplicate mods:\n\
                   \tMod ID: 'jei' from mod files: jei-19.1.jar, jei-19.2.jar\n";
        let Some(Cause::Duplicates(found)) = one(TextKind::Log, log, &ModIndex::default()) else {
            panic!();
        };
        assert_eq!(found[0].culprit.id, "jei");
        assert_eq!(found[0].files, ["jei-19.1.jar", "jei-19.2.jar"]);

        let issue = "\tFailure message: Mod §ejei§r is present in multiple files: a.jar, b.jar\n";
        let Some(Cause::Duplicates(found)) = report(issue) else {
            panic!();
        };
        assert_eq!(found[0].culprit.id, "jei");
        assert_eq!(found[0].files.len(), 2);
    }

    #[test]
    fn jvm_crashes_name_the_driver_or_library() {
        let hs_err = |frame: &str| {
            format!(
                "#\n# A fatal error has been detected by the Java Runtime Environment:\n#\n\
                 #  EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb, pid=1234, tid=5678\n#\n\
                 # Problematic frame:\n{frame}\n#\n"
            )
        };
        let jvm = |frame: &str| one(TextKind::Jvm, &hs_err(frame), &ModIndex::default());
        assert_eq!(
            jvm("# C  [nvoglv64.dll+0xabc]"),
            Some(Cause::Native(Native::Driver(Some(Vendor::Nvidia))))
        );
        assert_eq!(
            jvm("# C  [libnvidia-glcore.so.550.54.14+0x1234]"),
            Some(Cause::Native(Native::Driver(Some(Vendor::Nvidia))))
        );
        assert_eq!(
            jvm("# C  [atio6axx.dll+0x1f]"),
            Some(Cause::Native(Native::Driver(Some(Vendor::Amd))))
        );
        assert_eq!(
            jvm("# C  [radeonsi_dri.so+0x1f]"),
            Some(Cause::Native(Native::Driver(Some(Vendor::Amd))))
        );
        assert_eq!(
            jvm("# C  [libglfw.so+0x2a]  _glfwPlatformPollEvents+0x1a"),
            Some(Cause::Native(Native::Library("libglfw.so".into())))
        );
        assert_eq!(
            jvm("# V  [libjvm.so+0x5c1f20]  G1ParScanThreadState::trim_queue()"),
            Some(Cause::Native(Native::Java))
        );
        assert_eq!(
            jvm("# J 4567 c2 net.minecraft.world.level.Level.tick()V (40 bytes)"),
            Some(Cause::Native(Native::Java))
        );
    }

    #[test]
    fn what_java_prints_before_any_log_is_read_from_the_output() {
        let output = |text: &str| one(TextKind::Output, text, &ModIndex::default());
        assert_eq!(
            output(
                "Error occurred during initialization of VM\nCould not reserve enough space for 16777216KB object heap"
            ),
            Some(Cause::HeapSize)
        );
        assert_eq!(
            output(
                "# There is insufficient memory for the Java Runtime Environment to continue.\n# Native memory allocation (mmap) failed to map 1048576 bytes"
            ),
            Some(Cause::SystemMemory)
        );
        assert_eq!(
            output(
                "Exception in thread \"main\" java.lang.ClassCastException: class jdk.internal.loader.ClassLoaders$AppClassLoader cannot be cast to class java.net.URLClassLoader"
            ),
            Some(Cause::JavaTooNew)
        );
    }

    #[test]
    fn graphics_disk_and_broken_files() {
        let glfw = "java.lang.IllegalStateException: Failed to initialize GLFW, errors: GLFW error during init: [0x1000E]Failed to detect any supported platform\n\
                    \tat com.mojang.blaze3d.platform.GLX._initGlfw(SourceFile:77)\n";
        assert_eq!(report(glfw), Some(Cause::Graphics));

        let disk = "java.io.IOException: No space left on device\n\tat java.base/java.io.FileOutputStream.writeBytes(Native Method)\n";
        assert_eq!(report(disk), Some(Cause::DiskFull));

        let broken = "\tFailure message: File mods/create-6.0.jar is not a jar file\n";
        assert_eq!(
            report(broken),
            Some(Cause::Corrupt {
                file: Some("create-6.0.jar".into())
            })
        );
    }

    #[test]
    fn broken_configs_name_the_file_and_mod() {
        let text = "net.minecraftforge.fml.config.ConfigFileTypeHandler$ConfigLoadingException: Failed loading config file create-client.toml of type CLIENT for modid create\n\
                    Caused by: com.electronwill.nightconfig.core.io.ParsingException: Not enough data available\n";
        let index = index(&[("create", "Create", &[])]);
        let Some(Cause::Config { file, culprit }) = one(TextKind::Report, text, &index) else {
            panic!();
        };
        assert_eq!(file.as_deref(), Some("create-client.toml"));
        assert_eq!(
            culprit.map(|c| c.label().to_owned()).as_deref(),
            Some("Create")
        );
    }

    #[test]
    fn the_ticking_entity_or_block_is_named_with_its_place() {
        let entity = "Description: Ticking entity\n\n\
                      java.lang.NullPointerException: null\n\
                      \tat net.minecraft.world.entity.Mob.tick(Mob.java:1)\n\n\
                      -- Entity being ticked --\n\
                      Details:\n\
                      \tEntity Type: minecraft:zombie (net.minecraft.world.entity.monster.Zombie)\n\
                      \tEntity's Exact location: 12.50, 64.00, -3.20\n\
                      -- Affected level --\n";
        assert_eq!(
            report(entity),
            Some(Cause::Ticking(Ticking {
                kind: TickingKind::Entity,
                id: "minecraft:zombie".into(),
                at: Some("12, 64, -4".into()),
            }))
        );

        let block = "-- Block entity being ticked --\n\
                     Details:\n\
                     \tName: create:mechanical_press // com.simibubi.create.content.kinetics.press.MechanicalPressBlockEntity\n\
                     \tBlock: Block{create:mechanical_press}[facing=north]\n\
                     \tBlock location: World: (10,64,-5), Section: (at 10,0,11 in 0,4,-1; chunk contains blocks 0,-64,-16 to 15,319,-1)\n";
        let found = ticking(block).unwrap();
        assert_eq!(found.id, "create:mechanical_press");
        assert_eq!(found.at.as_deref(), Some("10, 64, -5"));
    }

    #[test]
    fn a_dependency_error_in_the_log_outranks_the_report() {
        let report_text = "java.lang.IllegalStateException: boom\n\tat TRANSFORMER/ponder@1.0/net.createmod.Foo.bar(Foo.java:1)\n";
        let log = "Missing or unsupported mandatory dependencies:\n\
                   \tMod ID: 'create', Requested by: 'ponder', Expected range: '[6.0.0,)', Actual version: '[MISSING]'\n";
        let found = analyze(
            &[(TextKind::Report, report_text), (TextKind::Log, log)],
            &ModIndex::default(),
        );
        assert!(matches!(found, Some((Cause::Dependencies(_), 1))));
    }
}
