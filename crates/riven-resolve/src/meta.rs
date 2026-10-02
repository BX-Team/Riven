use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use riven_format::{LoaderKind, Side};
use serde::Deserialize;

use crate::version::{ModVersion, VersionReq};

const MAX_NESTING: usize = 4;

/// The mod platform a metadata file was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl Platform {
    pub fn runs_on(self, loader: LoaderKind) -> bool {
        matches!(
            (self, loader),
            (Platform::Fabric, LoaderKind::Fabric | LoaderKind::Quilt)
                | (Platform::Quilt, LoaderKind::Quilt)
                | (Platform::Forge, LoaderKind::Forge)
                | (Platform::NeoForge, LoaderKind::NeoForge)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Required,
    Optional,
    Incompatible,
    Discouraged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDep {
    pub id: String,
    pub req: VersionReq,
    pub kind: DepKind,
    pub side: Side,
}

#[derive(Debug, Clone)]
pub struct ModMeta {
    pub platform: Platform,
    pub id: String,
    pub version: ModVersion,
    pub name: Option<String>,
    pub provides: Vec<String>,
    /// Declared environment, when the metadata states one.
    pub side: Option<Side>,
    pub deps: Vec<ModDep>,
}

/// Mod metadata of a jar, including jar-in-jar libraries.
#[derive(Debug, Clone, Default)]
pub struct JarMeta {
    pub mods: Vec<ModMeta>,
    pub nested: Vec<JarMeta>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("not a valid jar: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("cannot read {file}: {message}")]
    Parse { file: String, message: String },
}

impl JarMeta {
    pub fn read(bytes: &[u8]) -> Result<Self, MetaError> {
        read_jar(bytes, 0)
    }

    /// Mods loadable by `loader`, nested ones included.
    pub fn mods_for(&self, loader: LoaderKind) -> Vec<&ModMeta> {
        let mut out: Vec<&ModMeta> = self
            .mods
            .iter()
            .filter(|m| m.platform.runs_on(loader))
            .collect();
        for nested in &self.nested {
            out.extend(nested.mods_for(loader));
        }
        out
    }
}

fn read_jar(bytes: &[u8], depth: usize) -> Result<JarMeta, MetaError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut file = zip.by_name(name).ok()?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    let parse_err = |file: &str, message: String| MetaError::Parse {
        file: file.to_owned(),
        message,
    };

    let mut meta = JarMeta::default();
    let mut nested_paths = Vec::new();

    if let Some(raw) = read("fabric.mod.json") {
        let (m, jars) = fabric(&text(&raw)).map_err(|e| parse_err("fabric.mod.json", e))?;
        meta.mods.push(m);
        nested_paths.extend(jars);
    }
    if let Some(raw) = read("quilt.mod.json") {
        let (m, jars) = quilt(&text(&raw)).map_err(|e| parse_err("quilt.mod.json", e))?;
        meta.mods.push(m);
        nested_paths.extend(jars);
    }
    let jar_version = read("META-INF/MANIFEST.MF")
        .and_then(|raw| manifest_value(&text(&raw), "Implementation-Version"));
    for (file, platform) in [
        ("META-INF/neoforge.mods.toml", Some(Platform::NeoForge)),
        ("META-INF/mods.toml", None),
    ] {
        if let Some(raw) = read(file) {
            let mods = mods_toml(&text(&raw), platform, jar_version.as_deref())
                .map_err(|e| parse_err(file, e))?;
            meta.mods.extend(mods);
        }
    }
    if let Some(raw) = read("META-INF/jarjar/metadata.json") {
        #[derive(Deserialize)]
        struct JarJar {
            jars: Vec<JarJarEntry>,
        }
        #[derive(Deserialize)]
        struct JarJarEntry {
            path: String,
        }
        let jarjar: JarJar = serde_json::from_str(&text(&raw))
            .map_err(|e| parse_err("META-INF/jarjar/metadata.json", e.to_string()))?;
        nested_paths.extend(jarjar.jars.into_iter().map(|j| j.path));
    }

    if depth < MAX_NESTING {
        for path in nested_paths {
            // Nested libraries without mod metadata (or broken ones) contribute nothing.
            if let Some(inner) = read(&path)
                && let Ok(inner) = read_jar(&inner, depth + 1)
                && !(inner.mods.is_empty() && inner.nested.is_empty())
            {
                meta.nested.push(inner);
            }
        }
    }
    Ok(meta)
}

fn text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

fn manifest_value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_owned())
    })
}

/// Fabric Loader tolerates raw control characters inside JSON strings; serde_json does not.
fn lenient_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            } else if c.is_control() {
                out.push_str(match c {
                    '\n' => "\\n",
                    '\t' => "\\t",
                    _ => " ",
                });
                continue;
            }
        } else if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

fn env_side(value: Option<&str>) -> Option<Side> {
    match value? {
        "client" => Some(Side::Client),
        "server" | "dedicated_server" => Some(Side::Server),
        "*" => Some(Side::Both),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

fn fabric(input: &str) -> Result<(ModMeta, Vec<String>), String> {
    #[derive(Deserialize)]
    struct FabricMod {
        id: String,
        version: String,
        name: Option<String>,
        #[serde(default)]
        provides: Vec<String>,
        environment: Option<String>,
        #[serde(default)]
        depends: BTreeMap<String, OneOrMany>,
        #[serde(default)]
        recommends: BTreeMap<String, OneOrMany>,
        #[serde(default)]
        suggests: BTreeMap<String, OneOrMany>,
        #[serde(default)]
        breaks: BTreeMap<String, OneOrMany>,
        #[serde(default)]
        conflicts: BTreeMap<String, OneOrMany>,
        #[serde(default)]
        jars: Vec<FabricJar>,
    }
    #[derive(Deserialize)]
    struct FabricJar {
        file: String,
    }

    let m: FabricMod = serde_json::from_str(&lenient_json(input)).map_err(|e| e.to_string())?;
    let mut deps = Vec::new();
    for (map, kind) in [
        (m.depends, DepKind::Required),
        (m.recommends, DepKind::Optional),
        (m.suggests, DepKind::Optional),
        (m.breaks, DepKind::Incompatible),
        (m.conflicts, DepKind::Discouraged),
    ] {
        for (id, preds) in map {
            let req = VersionReq::fabric(&preds.into_vec()).map_err(|e| e.to_string())?;
            deps.push(ModDep {
                id,
                req,
                kind,
                side: Side::Both,
            });
        }
    }
    let meta = ModMeta {
        platform: Platform::Fabric,
        version: ModVersion::parse(&m.version),
        id: m.id,
        name: m.name,
        provides: m.provides,
        side: env_side(m.environment.as_deref()),
        deps,
    };
    Ok((meta, m.jars.into_iter().map(|j| j.file).collect()))
}

fn quilt(input: &str) -> Result<(ModMeta, Vec<String>), String> {
    #[derive(Deserialize)]
    struct Root {
        quilt_loader: Loader,
        minecraft: Option<Minecraft>,
    }
    #[derive(Deserialize)]
    struct Minecraft {
        environment: Option<String>,
    }
    #[derive(Deserialize)]
    struct Loader {
        id: String,
        version: String,
        metadata: Option<Metadata>,
        #[serde(default)]
        provides: Vec<Provide>,
        #[serde(default)]
        depends: Vec<Dep>,
        #[serde(default)]
        breaks: Vec<Dep>,
        #[serde(default)]
        jars: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Metadata {
        name: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Provide {
        Id(String),
        Full { id: String },
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Dep {
        Id(String),
        Full {
            id: String,
            versions: Option<serde_json::Value>,
            #[serde(default)]
            optional: bool,
        },
    }

    let root: Root = serde_json::from_str(&lenient_json(input)).map_err(|e| e.to_string())?;
    let l = root.quilt_loader;
    let bare_id = |id: &str| id.rsplit(':').next().unwrap_or(id).to_owned();
    let dep = |d: Dep, kind: DepKind| -> Result<ModDep, String> {
        let (id, versions, optional) = match d {
            Dep::Id(id) => (id, None, false),
            Dep::Full {
                id,
                versions,
                optional,
            } => (id, versions, optional),
        };
        let preds: Vec<String> = match versions {
            Some(serde_json::Value::String(s)) => vec![s],
            Some(serde_json::Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => vec![],
        };
        Ok(ModDep {
            id: bare_id(&id),
            req: VersionReq::fabric(&preds).map_err(|e| e.to_string())?,
            kind: if optional && kind == DepKind::Required {
                DepKind::Optional
            } else {
                kind
            },
            side: Side::Both,
        })
    };
    let mut deps = Vec::new();
    for d in l.depends {
        deps.push(dep(d, DepKind::Required)?);
    }
    for d in l.breaks {
        deps.push(dep(d, DepKind::Incompatible)?);
    }
    let meta = ModMeta {
        platform: Platform::Quilt,
        id: l.id,
        version: ModVersion::parse(&l.version),
        name: l.metadata.and_then(|m| m.name),
        provides: l
            .provides
            .into_iter()
            .map(|p| match p {
                Provide::Id(id) | Provide::Full { id } => bare_id(&id),
            })
            .collect(),
        side: env_side(root.minecraft.and_then(|m| m.environment).as_deref()),
        deps,
    };
    Ok((meta, l.jars))
}

fn mods_toml(
    input: &str,
    platform: Option<Platform>,
    jar_version: Option<&str>,
) -> Result<Vec<ModMeta>, String> {
    #[derive(Deserialize)]
    struct ModsToml {
        #[serde(default)]
        mods: Vec<TomlMod>,
        #[serde(default)]
        dependencies: BTreeMap<String, Vec<TomlDep>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TomlMod {
        mod_id: String,
        version: Option<String>,
        display_name: Option<String>,
        #[serde(default)]
        provides: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TomlDep {
        mod_id: String,
        mandatory: Option<bool>,
        #[serde(rename = "type")]
        kind: Option<String>,
        version_range: Option<String>,
        side: Option<String>,
    }

    let mut doc: ModsToml = toml::from_str(input).map_err(|e| e.to_string())?;
    // NeoForge 20.x still shipped `mods.toml`; its mods depend on `neoforge`.
    let platform = platform.unwrap_or_else(|| {
        let on_neoforge = doc
            .dependencies
            .values()
            .flatten()
            .any(|d| d.mod_id == "neoforge");
        if on_neoforge {
            Platform::NeoForge
        } else {
            Platform::Forge
        }
    });

    doc.mods
        .into_iter()
        .map(|m| {
            let version = match m.version.as_deref() {
                Some("${file.jarVersion}") | None => jar_version.unwrap_or("0").to_owned(),
                Some(v) => v.to_owned(),
            };
            let deps = doc
                .dependencies
                .remove(&m.mod_id)
                .unwrap_or_default()
                .into_iter()
                .map(|d| {
                    let kind = match d.kind.as_deref().map(str::to_ascii_lowercase).as_deref() {
                        Some("required") => DepKind::Required,
                        Some("optional") => DepKind::Optional,
                        Some("incompatible") => DepKind::Incompatible,
                        Some("discouraged") => DepKind::Discouraged,
                        _ if d.mandatory == Some(false) => DepKind::Optional,
                        _ => DepKind::Required,
                    };
                    let side = match d.side.as_deref().map(str::to_ascii_uppercase).as_deref() {
                        Some("CLIENT") => Side::Client,
                        Some("SERVER") => Side::Server,
                        _ => Side::Both,
                    };
                    let req = VersionReq::maven(d.version_range.as_deref().unwrap_or(""))
                        .map_err(|e| e.to_string())?;
                    Ok(ModDep {
                        id: d.mod_id,
                        req,
                        kind,
                        side,
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(ModMeta {
                platform,
                id: m.mod_id,
                version: ModVersion::parse(&version),
                name: m.display_name,
                provides: m.provides,
                side: None,
                deps,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    pub(crate) fn jar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    pub(crate) fn neoforge_jar(id: &str, version: &str, deps: &str) -> Vec<u8> {
        let toml = format!(
            "modLoader = \"javafml\"\nloaderVersion = \"[1,)\"\n[[mods]]\nmodId = \"{id}\"\nversion = \"{version}\"\n{deps}"
        );
        jar(&[("META-INF/neoforge.mods.toml", toml.as_bytes())])
    }

    fn v(s: &str) -> ModVersion {
        ModVersion::parse(s)
    }

    #[test]
    fn neoforge_with_quoted_keys_and_jarjar() {
        let toml = r#"
modLoader = "javafml"
loaderVersion = "[0,)"
license = "MIT"

[[mods]]
modId = "create"
version = "6.0.10"
displayName = "Create"

[[mixins]]
config = "create.mixins.json"

[[dependencies."create"]]
modId = "neoforge"
type = "required"
versionRange = "[21.1.219,)"
ordering = "NONE"
side = "BOTH"

[[dependencies."create"]]
modId = "flywheel"
type = "required"
versionRange = "[1.0.0,2.0)"
side = "CLIENT"

[[dependencies."create"]]
modId = "radium"
type = "incompatible"
side = "BOTH"
"#;
        let jarjar = r#"{ "jars": [
            { "path": "META-INF/jarjar/flywheel.jar" },
            { "path": "META-INF/jarjar/Registrate.jar" },
            { "path": "META-INF/jarjar/missing.jar" } ] }"#;
        let flywheel = neoforge_jar("flywheel", "1.0.6", "");
        let registrate = jar(&[("com/tterrag/Registrate.class", b"")]);
        let bytes = jar(&[
            ("META-INF/neoforge.mods.toml", toml.as_bytes()),
            ("META-INF/jarjar/metadata.json", jarjar.as_bytes()),
            ("META-INF/jarjar/flywheel.jar", &flywheel),
            ("META-INF/jarjar/Registrate.jar", &registrate),
        ]);

        let meta = JarMeta::read(&bytes).unwrap();
        let ids: Vec<_> = meta
            .mods_for(LoaderKind::NeoForge)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["create", "flywheel"]);
        assert!(meta.mods_for(LoaderKind::Fabric).is_empty());
        assert!(meta.mods_for(LoaderKind::Forge).is_empty());

        let create = &meta.mods[0];
        assert_eq!(create.platform, Platform::NeoForge);
        let flywheel = create.deps.iter().find(|d| d.id == "flywheel").unwrap();
        assert_eq!(
            (flywheel.kind, flywheel.side),
            (DepKind::Required, Side::Client)
        );
        assert!(!flywheel.req.matches(&v("2.0")));
        let radium = create.deps.iter().find(|d| d.id == "radium").unwrap();
        assert_eq!(radium.kind, DepKind::Incompatible);
        assert!(radium.req.is_any());
    }

    #[test]
    fn bare_version_range_is_only_a_recommendation() {
        let bytes = neoforge_jar(
            "sodium",
            "0.8.13+mc1.21.1",
            "provides = [\"indium\"]\n[[dependencies.sodium]]\nmodId = \"minecraft\"\ntype = \"required\"\nversionRange = \"1.21.1\"\n",
        );
        let meta = JarMeta::read(&bytes).unwrap();
        let sodium = &meta.mods[0];
        assert_eq!(sodium.provides, ["indium"]);
        assert!(sodium.deps[0].req.matches(&v("1.21.4")));
    }

    #[test]
    fn forge_jar_version_and_legacy_mandatory() {
        let toml = r#"
modLoader="javafml"
loaderVersion="[47,)"
[[mods]]
modId="jei"
version="${file.jarVersion}"
[[dependencies.jei]]
    modId="forge"
    mandatory=true
    versionRange="[47.1,)"
    side="BOTH"
[[dependencies.jei]]
    modId="kubejs"
    mandatory=false
    versionRange="[2001,)"
    side="client"
"#;
        let manifest = "Manifest-Version: 1.0\r\nImplementation-Version: 15.20.0.106\r\n";
        let bytes = jar(&[
            ("META-INF/mods.toml", toml.as_bytes()),
            ("META-INF/MANIFEST.MF", manifest.as_bytes()),
        ]);
        let meta = JarMeta::read(&bytes).unwrap();
        let jei = &meta.mods[0];
        assert_eq!(jei.platform, Platform::Forge);
        assert_eq!(jei.version, v("15.20.0.106"));
        assert_eq!(jei.deps[0].kind, DepKind::Required);
        assert_eq!(
            (jei.deps[1].kind, jei.deps[1].side),
            (DepKind::Optional, Side::Client)
        );
    }

    #[test]
    fn neoforge_in_legacy_mods_toml() {
        let toml = "modLoader=\"javafml\"\nloaderVersion=\"[1,)\"\n[[mods]]\nmodId=\"x\"\nversion=\"1\"\n[[dependencies.x]]\nmodId=\"neoforge\"\ntype=\"required\"\nversionRange=\"[20.4,)\"\n";
        let meta = JarMeta::read(&jar(&[("META-INF/mods.toml", toml.as_bytes())])).unwrap();
        assert_eq!(meta.mods[0].platform, Platform::NeoForge);
    }

    #[test]
    fn fabric_lenient_json_arrays_and_nested_jars() {
        let nested = jar(&[(
            "fabric.mod.json",
            br#"{"schemaVersion":1,"id":"fabric-api-base","version":"0.4.42"}"#,
        )]);
        let json = "\u{feff}{\n  \"schemaVersion\": 1,\n  \"id\": \"iris\",\n  \"version\": \"1.8.14-beta.1+mc1.21.1\",\n  \"description\": \"line one\nline two\",\n  \"environment\": \"client\",\n  \"depends\": { \"fabricloader\": \"\\u003e\\u003d0.12.3\", \"minecraft\": [\"1.21.1\"], \"sodium\": [\"0.8.x\"] },\n  \"breaks\": { \"embeddium\": \"*\", \"colormatic\": \"\\u003c\\u003d3.0.0\" },\n  \"jars\": [ { \"file\": \"META-INF/jars/base.jar\" }, { \"file\": \"META-INF/jars/antlr.jar\" } ]\n}";
        let bytes = jar(&[
            ("fabric.mod.json", json.as_bytes()),
            ("META-INF/jars/base.jar", &nested),
            (
                "META-INF/jars/antlr.jar",
                &jar(&[("org/antlr/X.class", b"")]),
            ),
        ]);
        let meta = JarMeta::read(&bytes).unwrap();
        let iris = &meta.mods[0];
        assert_eq!(iris.side, Some(Side::Client));
        let sodium = iris.deps.iter().find(|d| d.id == "sodium").unwrap();
        assert!(sodium.req.matches(&v("0.8.13+mc1.21.1")));
        assert!(!sodium.req.matches(&v("0.9.0")));
        let loader = iris.deps.iter().find(|d| d.id == "fabricloader").unwrap();
        assert!(loader.req.matches(&v("0.16.10")));
        let colormatic = iris.deps.iter().find(|d| d.id == "colormatic").unwrap();
        assert_eq!(colormatic.kind, DepKind::Incompatible);
        assert!(colormatic.req.matches(&v("3.0.0")));
        let ids: Vec<_> = meta
            .mods_for(LoaderKind::Quilt)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["iris", "fabric-api-base"]);
    }

    #[test]
    fn quilt_dependency_objects() {
        let json = r#"{ "schema_version": 1, "quilt_loader": {
            "group": "org.example", "id": "example", "version": "2.0.0",
            "metadata": { "name": "Example" },
            "provides": ["example_api", { "id": "org.example:legacy" }],
            "depends": [ "quilt_loader", { "id": "minecraft", "versions": ">=1.21" },
                         { "id": "qsl", "versions": ["1.0.x", "2.0.x"], "optional": true } ],
            "breaks": [ { "id": "bad", "versions": "<2" } ] },
            "minecraft": { "environment": "*" } }"#;
        let meta = JarMeta::read(&jar(&[("quilt.mod.json", json.as_bytes())])).unwrap();
        let m = &meta.mods[0];
        assert_eq!(m.provides, ["example_api", "legacy"]);
        assert_eq!(m.side, Some(Side::Both));
        let qsl = m.deps.iter().find(|d| d.id == "qsl").unwrap();
        assert_eq!(qsl.kind, DepKind::Optional);
        assert!(qsl.req.matches(&v("2.0.3")) && !qsl.req.matches(&v("3.0")));
        assert_eq!(
            m.deps.iter().find(|d| d.id == "bad").unwrap().kind,
            DepKind::Incompatible
        );
        assert!(meta.mods_for(LoaderKind::Fabric).is_empty());
    }

    #[test]
    fn plain_jar_has_no_mods() {
        let meta = JarMeta::read(&jar(&[("a/B.class", b"")])).unwrap();
        assert!(meta.mods.is_empty());
        assert!(JarMeta::read(b"not a zip").is_err());
    }
}
