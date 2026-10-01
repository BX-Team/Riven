use std::path::{Path, PathBuf};

use super::*;

/// Compares with a committed file; `RIVEN_BLESS=1` rewrites it instead.
fn golden(path: PathBuf, actual: &str) {
    if std::env::var_os("RIVEN_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with RIVEN_BLESS=1", path.display()));
    assert_eq!(
        expected,
        actual,
        "{} is stale; run with RIVEN_BLESS=1",
        path.display()
    );
}

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn entry(id: &str, path: &str, requires: &[&str]) -> Entry {
    Entry {
        id: id.into(),
        kind: Kind::Mod,
        name: id.into(),
        source: Source::Modrinth {
            project: "AANobbMI".into(),
            version: "Yp8wLY1P".into(),
        },
        file: EntryFile {
            path: PackPath::new(path).unwrap(),
            size: 1024,
            hashes: Hashes {
                sha512: Some("a".repeat(128)),
                sha1: Some("b".repeat(40)),
            },
            url: Some(format!("https://cdn.modrinth.com/{path}")),
        },
        side: Side::Both,
        group: None,
        update: UpdatePolicy::Follow,
        reason: Reason::Explicit,
        requires: requires.iter().map(|s| s.to_string()).collect(),
    }
}

fn project() -> Project {
    Project {
        name: "VideCraft: Create".into(),
        id: "videcraft-create".into(),
        version: "1.4.0".into(),
        authors: vec!["BX Team".into()],
        description: Some("Модовый сервер VideCraft".into()),
        icon: Some(PackPath::new("icon.png").unwrap()),
        minecraft: "1.21.1".into(),
        loader: Loader {
            kind: LoaderKind::NeoForge,
            version: "21.1.77".into(),
        },
        java: Java {
            major: 21,
            memory: Some(Memory {
                min: "4G".into(),
                recommended: "8G".into(),
            }),
            jvm_args: vec![],
        },
        groups: vec![
            Group {
                id: "velocity".into(),
                name: "Velocity forwarding".into(),
                description: None,
                default: false,
            },
            Group {
                id: "shaders".into(),
                name: "Шейдеры".into(),
                description: Some("Iris + Complementary".into()),
                default: false,
            },
        ],
        files: FileRules {
            preserve: vec!["options.txt".into(), "config/xaero/**".into()],
            ignore: vec!["**/*.bak".into()],
        },
        content: vec![
            entry("sodium", "mods/sodium.jar", &[]),
            entry("create", "mods/create.jar", &["ponder", "flywheel"]),
            Entry {
                source: Source::GitHub {
                    repo: "mezz/JustEnoughItems".into(),
                    tag: "19.21.0".into(),
                    asset: "jei-*.jar".into(),
                },
                file: EntryFile {
                    url: Some(
                        "https://github.com/mezz/JustEnoughItems/releases/download/19.21.0/jei.jar"
                            .into(),
                    ),
                    hashes: Hashes {
                        sha512: None,
                        sha1: Some("c".repeat(40)),
                    },
                    ..entry("jei", "mods/jei.jar", &[]).file
                },
                side: Side::Client,
                group: Some("shaders".into()),
                update: UpdatePolicy::Pinned,
                reason: Reason::Dependency,
                ..entry("jei", "mods/jei.jar", &[])
            },
            entry("flywheel", "mods/flywheel.jar", &[]),
            entry("ponder", "mods/ponder.jar", &[]),
        ],
    }
}

#[test]
fn project_writer_is_deterministic() {
    let project = project();
    let written = to_string(&project);
    golden(crate_dir().join("fixtures/project.json"), &written);

    let parsed: Project = from_str(&written).unwrap();
    assert_eq!(to_string(&parsed), written);

    let mut shuffled = parsed;
    shuffled.content.reverse();
    shuffled.groups.reverse();
    assert_eq!(to_string(&shuffled), written);
    assert!(project.validate().is_empty(), "{:?}", project.validate());
}

#[test]
fn committed_schemas_are_current() {
    let dir = crate_dir().join("../../schema/v1");
    golden(dir.join("project.json"), &schema::<Project>());
    golden(dir.join("release.json"), &schema::<Release>());
    golden(dir.join("channel.json"), &schema::<Channel>());
    golden(dir.join("state.json"), &schema::<State>());
}

#[test]
fn rejects_unknown_formats() {
    let newer = r#"{ "format": 2, "version": "1", "manifest": "x", "sha512": "x" }"#;
    assert!(matches!(
        from_str::<Channel>(newer),
        Err(Error::TooNew { found: 2, .. })
    ));
    let missing = r#"{ "version": "1", "manifest": "x", "sha512": "x" }"#;
    assert!(matches!(
        from_str::<Channel>(missing),
        Err(Error::MissingFormat(_))
    ));
    let zero = r#"{ "format": 0, "version": "1", "manifest": "x", "sha512": "x" }"#;
    assert!(matches!(
        from_str::<Channel>(zero),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn traversal_in_state_keys_is_rejected() {
    let state = r#"{ "format": 1, "source": "s", "side": "client", "version": "1",
        "files": { "../../.bashrc": { "sha512": "x" } } }"#;
    assert!(from_str::<State>(state).is_err());
}

#[test]
fn validate_reports_dangling_references() {
    let mut project = project();
    project
        .content
        .push(entry("sodium", "mods/sodium.jar", &["missing"]));
    project.content[0].group = Some("nope".into());
    let issues = project.validate();
    assert!(issues.contains(&Issue::DuplicateEntry("sodium".into())));
    assert!(issues.contains(&Issue::DuplicatePath(
        PackPath::new("mods/sodium.jar").unwrap()
    )));
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, Issue::UnknownGroup { .. }))
    );
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, Issue::UnknownRequire { require, .. } if require == "missing"))
    );
}
