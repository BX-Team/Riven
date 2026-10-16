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

/// A mod a crash points at; `name` and `file` are filled in from the instance's jars when found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Culprit {
    pub id: String,
    pub name: Option<String>,
    pub file: Option<PathBuf>,
}

impl Culprit {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: None,
            file: None,
        }
    }

    /// The mod's name when known, else its id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The loader refused to start the game.
    Dependencies(Vec<Dependency>),
    /// `culprit` calls code that is not there: of `other`, or of the game when `None`.
    Mismatch {
        culprit: Culprit,
        other: Option<Culprit>,
    },
    /// A mod could not patch the game.
    Mixin(Culprit),
    /// The game was started with an older Java than it needs.
    Java {
        needed: u32,
    },
    Memory,
    /// The crash happened inside a mod.
    Mod {
        culprit: Culprit,
        error: String,
    },
}

/// What most likely made a game stop, and the file that tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub cause: Cause,
    pub source: PathBuf,
}

/// Looks for why the game in `game_dir` stopped: a crash report written since `since`, then `logs/latest.log`.
/// With `crashed` false only a refusal to load is reported, since a clean exit has nothing else to explain.
pub fn diagnose(game_dir: &Path, since: SystemTime, crashed: bool) -> Option<Diagnosis> {
    let report = logs::list(game_dir)
        .into_iter()
        .find(|f| f.kind == logs::LogKind::Crash && f.modified >= since)
        .map(|f| f.path);
    let crashed = crashed || report.is_some();
    let log = game_dir.join("logs").join("latest.log");
    let fresh_log = std::fs::metadata(&log)
        .and_then(|m| m.modified())
        .is_ok_and(|m| m >= since);
    let sources = report
        .map(|r| (r, true))
        .into_iter()
        .chain(fresh_log.then_some((log, false)));
    let mut index = None;
    for (source, is_report) in sources {
        let Ok(text) = logs::read(&source) else {
            continue;
        };
        let index = index.get_or_insert_with(|| ModIndex::scan(&game_dir.join("mods")));
        if let Some(cause) = analyze(&text, index, is_report)
            && (crashed || matches!(cause, Cause::Dependencies(_)))
        {
            return Some(Diagnosis { cause, source });
        }
    }
    None
}

/// Mod ids, names and files of an instance, and which mod each class package belongs to.
#[derive(Debug, Default)]
pub struct ModIndex {
    mods: HashMap<String, (Option<String>, PathBuf)>,
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
            let mut jars = vec![&meta];
            while let Some(jar) = jars.pop() {
                for m in &jar.mods {
                    index
                        .mods
                        .entry(m.id.clone())
                        .or_insert_with(|| (m.name.clone(), path.clone()));
                }
                jars.extend(&jar.nested);
            }
            for name in zip.file_names().flatten() {
                if let Some((package, _)) = name.rsplit_once('/')
                    && name.ends_with(".class")
                    && !name.starts_with("META-INF/")
                {
                    index
                        .packages
                        .entry(package.replace('/', "."))
                        .or_insert_with(|| main.id.clone());
                }
            }
        }
        index
    }

    fn culprit(&self, id: &str) -> Culprit {
        let mut culprit = Culprit::new(id);
        if let Some((name, file)) = self.mods.get(id) {
            culprit.name = name.clone();
            culprit.file = Some(file.clone());
        }
        culprit
    }

    fn owner(&self, class: &str) -> Option<Culprit> {
        let (package, _) = class.rsplit_once('.')?;
        self.packages.get(package).map(|id| self.culprit(id))
    }
}

/// Finds the most likely cause in a crash report or, with `report` false, a game log.
/// Mods log exceptions they survive, so a log's stack traces never name a culprit.
pub fn analyze(text: &str, index: &ModIndex, report: bool) -> Option<Cause> {
    let found = dependencies(text, index);
    if !found.is_empty() {
        return Some(Cause::Dependencies(found));
    }
    if let Some(needed) = java_needed(text) {
        return Some(Cause::Java { needed });
    }
    let errors = exceptions(text);
    if errors
        .iter()
        .any(|e| e.header.contains("java.lang.OutOfMemoryError"))
    {
        return Some(Cause::Memory);
    }
    if let Some(id) = mixin_culprit(text) {
        return Some(Cause::Mixin(index.culprit(&id)));
    }
    if !report {
        return None;
    }
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
    None
}

/// Unmet requirements in NeoForge's and Fabric's words.
fn dependencies(text: &str, index: &ModIndex) -> Vec<Dependency> {
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
    out
}

/// `Mod ID: 'create', Requested by: 'createaddition', Expected range: '[6.0.0,)', Actual version: '[MISSING]'`.
fn neoforge_dependency(line: &str, index: &ModIndex) -> Option<Dependency> {
    let field = |name: &str| -> Option<&str> {
        let start = line.find(&format!("{name}: '"))? + name.len() + 3;
        let len = line[start..].find('\'')?;
        Some(&line[start..start + len])
    };
    let found = field("Actual version")?;
    Some(Dependency {
        by: index.culprit(field("Requested by")?),
        needs: index.culprit(field("Mod ID")?),
        range: field("Expected range")?.to_owned(),
        found: (found != "[MISSING]").then(|| found.to_owned()),
        incompatible: false,
    })
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
    let mut by = index.culprit(by_id);
    by.name.get_or_insert_with(|| by_name.to_owned());
    let mut needs = index.culprit(needs_id);
    if let Some(name) = needs_name {
        needs.name.get_or_insert_with(|| name.to_owned());
    }
    Some(Dependency {
        by,
        needs,
        range: range.to_owned(),
        found,
        incompatible,
    })
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

/// The mod named by a mixin failure: NeoForge's message, Mixin's own `from mod x`, Fabric's `Mixin apply for mod x failed`.
fn mixin_culprit(text: &str) -> Option<String> {
    for line in text.lines().filter(|l| !l.trim_start().starts_with("at ")) {
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

struct Exception<'a> {
    header: &'a str,
    frames: Vec<&'a str>,
}

/// The exception and its causes as they appear: a header line, then its `at` frames.
fn exceptions(text: &str) -> Vec<Exception<'_>> {
    let mut out: Vec<Exception> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(frame) = trimmed.strip_prefix("at ") {
            if let Some(last) = out.last_mut() {
                last.frames.push(frame);
            }
        } else if is_exception_header(trimmed) {
            out.push(Exception {
                header: trimmed.strip_prefix("Caused by: ").unwrap_or(trimmed),
                frames: Vec::new(),
            });
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
                (Some((*name).to_owned()), PathBuf::from(format!("{id}.jar"))),
            );
            for p in *packages {
                index.packages.insert((*p).to_owned(), (*id).to_owned());
            }
        }
        index
    }

    #[test]
    fn a_crash_inside_a_mod_names_it_not_the_game_around_it() {
        let index = index(&[("ponder", "Ponder", &[])]);
        let Some(Cause::Mod { culprit, error }) =
            analyze(&fixture("neoforge-npe.txt"), &index, true)
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
        let cause = analyze(&fixture("neoforge-nosuchmethod.txt"), &index, true);
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
        let cause = analyze(text, &ModIndex::default(), true);
        assert!(matches!(cause, Some(Cause::Mismatch { other: None, .. })));
    }

    #[test]
    fn fabric_frames_are_matched_by_package() {
        let text = "java.lang.IllegalStateException: boom\n\
                    \tat net.minecraft.class_310.method_1514(class_310.java:10)\n\
                    \tat me.jellysquid.mods.sodium.client.Render.draw(Render.java:5)\n";
        let index = index(&[("sodium", "Sodium", &["me.jellysquid.mods.sodium.client"])]);
        let Some(Cause::Mod { culprit, .. }) = analyze(text, &index, true) else {
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
        let Some(Cause::Dependencies(deps)) = analyze(log, &index, false) else {
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
        let Some(Cause::Dependencies(deps)) = analyze(log, &ModIndex::default(), false) else {
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
        assert_eq!(
            analyze(java, &ModIndex::default(), true),
            Some(Cause::Java { needed: 21 })
        );

        let memory = "Description: Unexpected error\n\njava.lang.OutOfMemoryError: Java heap space\n\tat java.base/java.util.Arrays.copyOf(Arrays.java:1)\n";
        assert_eq!(
            analyze(memory, &ModIndex::default(), true),
            Some(Cause::Memory)
        );

        let mixin = "org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered\n\
                     Caused by: org.spongepowered.asm.mixin.injection.throwables.InjectionError: Critical injection failure: @Inject annotation on render could not find any targets matching 'draw' in net.minecraft.class_757. Using refmap iris-refmap.json [PREINJECT Applicator Phase -> iris.mixins.json:MixinGameRenderer from mod iris -> Prepare Injections]\n";
        let Some(Cause::Mixin(culprit)) = analyze(mixin, &ModIndex::default(), true) else {
            panic!();
        };
        assert_eq!(culprit.id, "iris");
    }

    #[test]
    fn exceptions_a_log_survived_are_not_causes() {
        let log = "[12:00:00] [Render thread/ERROR] [mezz.jei.library.load.PluginCaller/]: Caught an error from mod plugin\n\
                   java.lang.IllegalArgumentException: Tried to register a recipe type twice\n\
                   \tat TRANSFORMER/jei@19.39.0/mezz.jei.library.recipes.RecipeTypeData.register(RecipeTypeData.java:20) ~[jei.jar:?]\n";
        let index = index(&[("jei", "Just Enough Items", &[])]);
        assert_eq!(analyze(log, &index, false), None);
        assert!(matches!(
            analyze(log, &index, true),
            Some(Cause::Mod { .. })
        ));
    }

    #[test]
    fn java_modules_are_not_mods() {
        let text = "java.lang.IllegalStateException: boom\n\
                    \tat TRANSFORMER/com.google.common@32.1.2-jre/com.google.common.base.Preconditions.checkState(Preconditions.java:1)\n";
        assert_eq!(analyze(text, &ModIndex::default(), true), None);
    }

    #[test]
    fn debug_log_mentions_and_applied_mixins_are_not_causes() {
        let noise = "[main/TRACE] [mixin/]: Added class metadata for java/lang/OutOfMemoryError to metadata cache\n\
                     \tat TRANSFORMER/minecraft@1.21.1/net.minecraft.Foo.bar(Foo.java) {re:mixin,pl:mixin:APP:lootjs.mixins.json:Mixin from mod lootjs}\n";
        assert_eq!(analyze(noise, &ModIndex::default(), false), None);
    }
}
