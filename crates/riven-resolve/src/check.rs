use std::collections::BTreeMap;
use std::fmt;

use riven_format::{InstallSide, LoaderKind, Side};

use crate::meta::{DepKind, JarMeta, ModDep};
use crate::version::{ModVersion, VersionReq};

/// The game the pack targets, from `riven.json`.
#[derive(Debug, Clone)]
pub struct Env {
    pub minecraft: ModVersion,
    pub loader: LoaderKind,
    pub loader_version: ModVersion,
    pub java: u32,
}

/// One pack entry with the metadata of its jar.
#[derive(Debug, Clone, Copy)]
pub struct Installed<'a> {
    pub entry: &'a str,
    pub side: Side,
    pub meta: &'a JarMeta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    WrongLoader {
        entry: String,
    },
    Missing {
        entry: String,
        dep: String,
        req: String,
        side: InstallSide,
    },
    Mismatch {
        entry: String,
        dep: String,
        req: String,
        found: String,
        provider: String,
    },
    Incompatible {
        entry: String,
        other: String,
        dep: String,
    },
    Duplicate {
        id: String,
        entries: Vec<String>,
    },
}

impl Problem {
    /// The pack entry the problem belongs to.
    pub fn entry(&self) -> &str {
        match self {
            Problem::WrongLoader { entry }
            | Problem::Missing { entry, .. }
            | Problem::Mismatch { entry, .. }
            | Problem::Incompatible { entry, .. } => entry,
            Problem::Duplicate { entries, .. } => &entries[0],
        }
    }
}

fn side_name(side: InstallSide) -> &'static str {
    match side {
        InstallSide::Client => "client",
        InstallSide::Server => "server",
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::WrongLoader { entry } => {
                write!(f, "`{entry}` is not built for the pack's loader")
            }
            Problem::Missing {
                entry,
                dep,
                req,
                side,
            } => write!(
                f,
                "`{entry}` requires `{dep}` {req} on the {}, which is missing",
                side_name(*side)
            ),
            Problem::Mismatch {
                entry,
                dep,
                req,
                found,
                provider,
            } => write!(
                f,
                "`{entry}` requires `{dep}` {req}, but `{provider}` provides {found}"
            ),
            Problem::Incompatible { entry, other, dep } if dep == other => {
                write!(f, "`{entry}` is incompatible with `{other}`")
            }
            Problem::Incompatible { entry, other, dep } => {
                write!(
                    f,
                    "`{entry}` is incompatible with `{dep}`, shipped by `{other}`"
                )
            }
            Problem::Duplicate { id, entries } => {
                write!(f, "mod `{id}` is shipped by {}", entries.join(", "))
            }
        }
    }
}

struct Provider<'a> {
    version: ModVersion,
    entry: &'a str,
}

/// Checks dependencies, incompatibilities and duplicates of a pack, offline.
pub fn check(env: &Env, installed: &[Installed<'_>]) -> Vec<Problem> {
    let mut problems = Vec::new();
    for item in installed {
        if item.meta.wrong_loader(env.loader) {
            problems.push(Problem::WrongLoader {
                entry: item.entry.to_owned(),
            });
        }
    }

    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for item in installed {
        for m in item
            .meta
            .mods
            .iter()
            .filter(|m| m.platform.runs_on(env.loader))
        {
            let list = owners.entry(&m.id).or_default();
            if !list.contains(&item.entry) {
                list.push(item.entry);
            }
        }
    }
    for (id, entries) in owners {
        if entries.len() > 1 {
            problems.push(Problem::Duplicate {
                id: id.to_owned(),
                entries: entries.iter().map(|e| e.to_string()).collect(),
            });
        }
    }

    for side in [InstallSide::Client, InstallSide::Server] {
        let present: Vec<&Installed> = installed.iter().filter(|i| i.side.includes(side)).collect();
        let providers = providers(env, &present);
        for item in &present {
            for m in item
                .meta
                .mods
                .iter()
                .filter(|m| m.platform.runs_on(env.loader))
            {
                for dep in m.deps.iter().filter(|d| d.side.includes(side)) {
                    if let Some(problem) = check_dep(item.entry, dep, side, &providers)
                        && !problems.contains(&problem)
                    {
                        problems.push(problem);
                    }
                }
            }
        }
    }
    problems
}

fn providers<'a>(env: &Env, present: &[&'a Installed<'a>]) -> BTreeMap<String, Provider<'a>> {
    let mut map: BTreeMap<String, Provider> = BTreeMap::new();
    let mut offer = |id: &str, version: &ModVersion, entry: &'a str| {
        let better = map.get(id).is_none_or(|p| *version > p.version);
        if better {
            map.insert(
                id.to_owned(),
                Provider {
                    version: version.clone(),
                    entry,
                },
            );
        }
    };

    offer("minecraft", &env.minecraft, "minecraft");
    offer("java", &ModVersion::parse(&env.java.to_string()), "java");
    let loader_ids: &[&str] = match env.loader {
        LoaderKind::Fabric => &["fabricloader"],
        LoaderKind::Quilt => &["quilt_loader"],
        LoaderKind::Forge => &["forge"],
        LoaderKind::NeoForge => &["neoforge"],
    };
    for id in loader_ids {
        offer(id, &env.loader_version, "loader");
    }

    for item in present {
        for m in item.meta.mods_for(env.loader) {
            offer(&m.id, &m.version, item.entry);
            for alias in &m.provides {
                offer(alias, &m.version, item.entry);
            }
        }
    }
    map
}

/// Ids every loader resolves internally; never reported missing.
fn builtin(id: &str) -> bool {
    matches!(id, "fml" | "javafml" | "mixinextras" | "quilt_base")
}

fn check_dep(
    entry: &str,
    dep: &ModDep,
    side: InstallSide,
    providers: &BTreeMap<String, Provider<'_>>,
) -> Option<Problem> {
    let provider = providers.get(&dep.id);
    let matches = |req: &VersionReq, p: &Provider| req.matches(&p.version);
    match (dep.kind, provider) {
        (DepKind::Required, None) if !builtin(&dep.id) && !quilt_fabric_loader(dep, providers) => {
            Some(Problem::Missing {
                entry: entry.to_owned(),
                dep: dep.id.clone(),
                req: dep.req.to_string(),
                side,
            })
        }
        (DepKind::Required | DepKind::Optional, Some(p)) if p.entry != entry => {
            (!matches(&dep.req, p)).then(|| Problem::Mismatch {
                entry: entry.to_owned(),
                dep: dep.id.clone(),
                req: dep.req.to_string(),
                found: p.version.to_string(),
                provider: p.entry.to_owned(),
            })
        }
        (DepKind::Incompatible, Some(p)) if p.entry != entry && matches(&dep.req, p) => {
            Some(Problem::Incompatible {
                entry: entry.to_owned(),
                other: p.entry.to_owned(),
                dep: dep.id.clone(),
            })
        }
        _ => None,
    }
}

/// Quilt Loader satisfies `fabricloader` for Fabric mods, under its own version scheme.
fn quilt_fabric_loader(dep: &ModDep, providers: &BTreeMap<String, Provider<'_>>) -> bool {
    dep.id == "fabricloader" && providers.contains_key("quilt_loader")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{jar, neoforge_jar};

    fn env() -> Env {
        Env {
            minecraft: ModVersion::parse("1.21.1"),
            loader: LoaderKind::NeoForge,
            loader_version: ModVersion::parse("21.1.77"),
            java: 21,
        }
    }

    fn meta(bytes: &[u8]) -> JarMeta {
        JarMeta::read(bytes).unwrap()
    }

    fn dep(id: &str, kind: &str, range: &str, side: &str) -> String {
        format!(
            "[[dependencies.{{ID}}]]\nmodId = \"{id}\"\ntype = \"{kind}\"\nversionRange = \"{range}\"\nside = \"{side}\"\n"
        )
    }

    fn mod_jar(id: &str, version: &str, deps: &[String]) -> JarMeta {
        let deps = deps.concat().replace("{ID}", id);
        meta(&neoforge_jar(id, version, &deps))
    }

    #[test]
    fn clean_pack_has_no_problems() {
        let create = mod_jar(
            "create",
            "6.0.10",
            &[
                dep("neoforge", "required", "[21.1.0,)", "BOTH"),
                dep("minecraft", "required", "[1.21.1]", "BOTH"),
                dep("ponder", "required", "[1.0.82,)", "BOTH"),
                dep("sodium", "optional", "[0.6.9,)", "CLIENT"),
                dep("radium", "incompatible", "", "BOTH"),
            ],
        );
        let ponder = mod_jar("ponder", "1.0.82+mc1.21.1", &[]);
        let sodium = mod_jar("sodium", "0.8.13", &[]);
        let pack = [
            Installed {
                entry: "create",
                side: Side::Both,
                meta: &create,
            },
            Installed {
                entry: "ponder",
                side: Side::Both,
                meta: &ponder,
            },
            Installed {
                entry: "sodium",
                side: Side::Client,
                meta: &sodium,
            },
        ];
        assert_eq!(check(&env(), &pack), []);
    }

    #[test]
    fn reports_missing_per_side_mismatch_and_incompatible() {
        let create = mod_jar(
            "create",
            "6.0.10",
            &[
                dep("neoforge", "required", "[21.1.219,)", "BOTH"),
                dep("flywheel", "required", "[1.0.0,2.0)", "CLIENT"),
                dep("ponder", "required", "[1.0.82,)", "BOTH"),
                dep("sodium", "optional", "[0.6.9,)", "CLIENT"),
                dep("radium", "incompatible", "", "BOTH"),
            ],
        );
        let ponder = mod_jar("ponder", "1.0.82", &[]);
        let old_sodium = mod_jar("sodium", "0.6.2", &[]);
        let radium = mod_jar("radium", "0.13", &[]);
        let pack = [
            Installed {
                entry: "create",
                side: Side::Both,
                meta: &create,
            },
            Installed {
                entry: "ponder",
                side: Side::Server,
                meta: &ponder,
            },
            Installed {
                entry: "sodium",
                side: Side::Client,
                meta: &old_sodium,
            },
            Installed {
                entry: "radium",
                side: Side::Both,
                meta: &radium,
            },
        ];
        let problems = check(&env(), &pack);
        let has = |p: &Problem| problems.contains(p);

        assert!(problems.iter().any(|p| matches!(p,
            Problem::Mismatch { dep, provider, .. } if dep == "neoforge" && provider == "loader")));
        assert!(has(&Problem::Missing {
            entry: "create".into(),
            dep: "flywheel".into(),
            req: "[1.0.0,2.0)".into(),
            side: InstallSide::Client,
        }));
        assert!(!problems.iter().any(|p| matches!(p,
            Problem::Missing { dep, side: InstallSide::Server, .. } if dep == "flywheel")));
        assert!(has(&Problem::Missing {
            entry: "create".into(),
            dep: "ponder".into(),
            req: "[1.0.82,)".into(),
            side: InstallSide::Client,
        }));
        assert!(problems.iter().any(|p| matches!(p,
            Problem::Mismatch { dep, found, .. } if dep == "sodium" && found == "0.6.2")));
        assert!(has(&Problem::Incompatible {
            entry: "create".into(),
            other: "radium".into(),
            dep: "radium".into(),
        }));
    }

    #[test]
    fn nested_and_provided_ids_satisfy_dependencies() {
        let flywheel = neoforge_jar("flywheel", "1.0.6", "");
        let create = meta(&jar(&[
            (
                "META-INF/neoforge.mods.toml",
                format!(
                    "modLoader=\"javafml\"\nloaderVersion=\"[1,)\"\n[[mods]]\nmodId=\"create\"\nversion=\"6\"\n{}{}",
                    dep("flywheel", "required", "[1.0.0,2.0)", "CLIENT"),
                    dep("indium", "required", "", "BOTH"),
                )
                .replace("{ID}", "create")
                .as_bytes(),
            ),
            (
                "META-INF/jarjar/metadata.json",
                br#"{"jars":[{"path":"META-INF/jarjar/flywheel.jar"}]}"#,
            ),
            ("META-INF/jarjar/flywheel.jar", &flywheel),
        ]));
        let sodium = meta(&neoforge_jar("sodium", "0.8", "provides = [\"indium\"]\n"));
        let pack = [
            Installed {
                entry: "create",
                side: Side::Both,
                meta: &create,
            },
            Installed {
                entry: "sodium",
                side: Side::Both,
                meta: &sodium,
            },
        ];
        assert_eq!(check(&env(), &pack), []);
    }

    #[test]
    fn wrong_loader_and_duplicates() {
        let fabric_only = meta(&jar(&[(
            "fabric.mod.json",
            br#"{"schemaVersion":1,"id":"lithium","version":"0.14"}"#,
        )]));
        let a = mod_jar("jei", "19.0", &[]);
        let b = mod_jar("jei", "19.1", &[]);
        let pack = [
            Installed {
                entry: "lithium",
                side: Side::Both,
                meta: &fabric_only,
            },
            Installed {
                entry: "jei",
                side: Side::Both,
                meta: &a,
            },
            Installed {
                entry: "jei-fork",
                side: Side::Client,
                meta: &b,
            },
        ];
        let problems = check(&env(), &pack);
        assert!(problems.contains(&Problem::WrongLoader {
            entry: "lithium".into()
        }));
        assert!(problems.contains(&Problem::Duplicate {
            id: "jei".into(),
            entries: vec!["jei".into(), "jei-fork".into()],
        }));
    }
}
