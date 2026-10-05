use std::collections::BTreeMap;

use serde::Deserialize;

use crate::os;

/// A version file in Mojang's launcher format; loaders ship theirs in the same shape.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    #[serde(default)]
    pub id: String,
    pub inherits_from: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub main_class: Option<String>,
    /// The single game argument string versions before 1.13 use.
    pub minecraft_arguments: Option<String>,
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub asset_index: Option<AssetIndexRef>,
    pub assets: Option<String>,
    pub downloads: Option<Downloads>,
    pub java_version: Option<JavaVersion>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
    /// Defaults Mojang's launcher puts in a new profile's JVM arguments.
    #[serde(default, rename = "default-user-jvm")]
    pub default_user_jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled {
        #[serde(default)]
        rules: Vec<Rule>,
        value: Values,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Values {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: Action,
    pub os: Option<OsRule>,
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
    pub version_range: Option<VersionRange>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionRange {
    pub max: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    /// The repository a library without `downloads` comes from.
    pub url: Option<String>,
    pub rules: Option<Vec<Rule>>,
    /// OS name → classifier of the jar holding this library's native code.
    pub natives: Option<BTreeMap<String, String>>,
    pub extract: Option<Extract>,
    /// Old Forge profiles mark server-only libraries with `false`.
    pub clientreq: Option<bool>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<BTreeMap<String, Artifact>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub path: Option<String>,
    /// Empty when a loader installer produces the file instead.
    #[serde(default)]
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Downloads {
    pub client: Option<Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
}

/// The launcher features rules ask about.
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    pub custom_resolution: bool,
}

impl VersionJson {
    /// Fills what this version leaves out from the one it inherits from, the way Mojang's launcher does.
    pub fn inherit(self, parent: VersionJson) -> VersionJson {
        let arguments = match (parent.arguments, self.arguments) {
            (Some(mut parent), Some(child)) => {
                parent.game.extend(child.game);
                parent.jvm.extend(child.jvm);
                if !child.default_user_jvm.is_empty() {
                    parent.default_user_jvm = child.default_user_jvm;
                }
                Some(parent)
            }
            (parent, child) => child.or(parent),
        };
        let mut libraries = self.libraries;
        libraries.extend(parent.libraries);
        VersionJson {
            id: self.id,
            inherits_from: None,
            kind: self.kind.or(parent.kind),
            main_class: self.main_class.or(parent.main_class),
            minecraft_arguments: self.minecraft_arguments.or(parent.minecraft_arguments),
            arguments,
            libraries,
            asset_index: self.asset_index.or(parent.asset_index),
            assets: self.assets.or(parent.assets),
            downloads: self.downloads.or(parent.downloads),
            java_version: self.java_version.or(parent.java_version),
        }
    }
}

/// Whether `rules` let something apply here; no rules means it always does.
pub fn allowed(rules: &[Rule], features: Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    rules.iter().fold(false, |allow, rule| {
        if rule.matches(features) {
            rule.action == Action::Allow
        } else {
            allow
        }
    })
}

impl Rule {
    fn matches(&self, features: Features) -> bool {
        if let Some(rule) = &self.os {
            if rule.name.as_deref().is_some_and(|name| name != os::NAME) {
                return false;
            }
            let arch_matches = match rule.arch.as_deref() {
                None => true,
                Some("x86") => cfg!(target_arch = "x86"),
                Some(arch) => arch == std::env::consts::ARCH,
            };
            // Version patterns and `max` bounds only single out OS releases older than we support.
            let old_os = rule.version.is_some()
                || rule.version_range.as_ref().is_some_and(|r| r.max.is_some());
            if !arch_matches || old_os {
                return false;
            }
        }
        self.features.iter().flatten().all(|(name, wanted)| {
            let have = match name.as_str() {
                "has_custom_resolution" => features.custom_resolution,
                _ => false,
            };
            have == *wanted
        })
    }
}

impl Argument {
    pub fn values(&self, features: Features) -> &[String] {
        match self {
            Argument::Plain(value) => std::slice::from_ref(value),
            Argument::Ruled { rules, value } if allowed(rules, features) => match value {
                Values::One(v) => std::slice::from_ref(v),
                Values::Many(v) => v,
            },
            Argument::Ruled { .. } => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(json: &str) -> Vec<Rule> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn later_rules_override_earlier_ones() {
        let all_but_mac =
            rules(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]"#);
        assert_eq!(
            allowed(&all_but_mac, Features::default()),
            os::NAME != "osx"
        );
        let only_here = format!(r#"[{{"action":"allow","os":{{"name":"{}"}}}}]"#, os::NAME);
        assert!(allowed(&rules(&only_here), Features::default()));
        let old_mac = rules(
            r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx","version":"^10\\.5\\.\\d$"}}]"#,
        );
        assert!(allowed(&old_mac, Features::default()));
    }

    #[test]
    fn features_gate_resolution_arguments() {
        let arg: Argument = serde_json::from_str(
            r#"{"rules":[{"action":"allow","features":{"has_custom_resolution":true}}],
                "value":["--width","${resolution_width}"]}"#,
        )
        .unwrap();
        assert!(arg.values(Features::default()).is_empty());
        let on = Features {
            custom_resolution: true,
        };
        assert_eq!(arg.values(on), ["--width", "${resolution_width}"]);
        let demo: Argument = serde_json::from_str(
            r#"{"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"}"#,
        )
        .unwrap();
        assert!(demo.values(on).is_empty());
    }

    #[test]
    fn children_append_arguments_and_libraries_and_override_the_rest() {
        let parent: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.1","type":"release","mainClass":"net.minecraft.client.main.Main",
                "arguments":{"game":["--username","${auth_player_name}"],"jvm":["-cp","${classpath}"]},
                "libraries":[{"name":"org.ow2.asm:asm:9.6"}],
                "assetIndex":{"id":"17","url":"https://example.invalid/17.json"},
                "javaVersion":{"majorVersion":21}}"#,
        )
        .unwrap();
        let child: VersionJson = serde_json::from_str(
            r#"{"id":"fabric-loader-0.16.14-1.21.1","inheritsFrom":"1.21.1",
                "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
                "arguments":{"game":[],"jvm":["-DFabricMcEmu= net.minecraft.client.main.Main "]},
                "libraries":[{"name":"org.ow2.asm:asm:9.8","url":"https://maven.fabricmc.net/"}]}"#,
        )
        .unwrap();
        let merged = child.inherit(parent);
        assert_eq!(merged.id, "fabric-loader-0.16.14-1.21.1");
        assert_eq!(
            merged.main_class.as_deref(),
            Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
        );
        let jvm: Vec<&str> = merged
            .arguments
            .as_ref()
            .unwrap()
            .jvm
            .iter()
            .flat_map(|a| a.values(Features::default()))
            .map(String::as_str)
            .collect();
        assert_eq!(
            jvm,
            [
                "-cp",
                "${classpath}",
                "-DFabricMcEmu= net.minecraft.client.main.Main "
            ]
        );
        let names: Vec<&str> = merged.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["org.ow2.asm:asm:9.8", "org.ow2.asm:asm:9.6"]);
        assert_eq!(merged.java_version.unwrap().major_version, 21);
        assert_eq!(merged.asset_index.unwrap().id, "17");
    }

    #[test]
    fn legacy_argument_strings_replace_the_parents() {
        let parent: VersionJson = serde_json::from_str(
            r#"{"id":"1.12.2","minecraftArguments":"--username ${auth_player_name}"}"#,
        )
        .unwrap();
        let child: VersionJson = serde_json::from_str(
            r#"{"id":"1.12.2-forge-14.23.5.2859","inheritsFrom":"1.12.2",
                "minecraftArguments":"--username ${auth_player_name} --tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker"}"#,
        )
        .unwrap();
        let merged = child.inherit(parent);
        assert!(merged.minecraft_arguments.unwrap().ends_with("FMLTweaker"));
    }
}
