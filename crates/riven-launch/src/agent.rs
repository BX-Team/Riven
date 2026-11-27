use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{LaunchError, io};

/// Built from `agent/` by `./gradlew build`, which writes it here.
const JAR: &[u8] = include_bytes!("../riven-agent.jar");

/// What Riven's agent needs to show BX ID skins in the game, from the skins service.
#[derive(Clone)]
pub struct SkinsAgent {
    /// The skins service, such as `https://api.bxteam.org/v1/skins`.
    pub api: String,
    /// The key that signs its textures: X.509, base64.
    pub key: String,
    /// For an offline account whose name is one of the BX ID's nicks: `join` then goes to Riven.
    pub game_token: Option<String>,
}

impl std::fmt::Debug for SkinsAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkinsAgent")
            .field("api", &self.api)
            .field(
                "game_token",
                &self.game_token.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl SkinsAgent {
    /// The JVM options that load the agent, after writing its jar under `data` if this build's is not there yet.
    /// `offline` sends the account's `join` to Riven; a Microsoft account's must stay with Mojang, token and all.
    pub fn jvm_args(&self, data: &Path, offline: bool) -> Result<Vec<String>, LaunchError> {
        let jar = write_jar(data)?;
        let mut args = vec![
            format!("-javaagent:{}", jar.display()),
            format!("-Driven.skins.api={}", self.api),
            format!("-Driven.skins.key={}", self.key),
        ];
        if offline && self.game_token.is_some() {
            args.push("-Driven.skins.join=true".into());
        }
        Ok(args)
    }
}

fn write_jar(data: &Path) -> Result<PathBuf, LaunchError> {
    let hash = hex::encode(&Sha256::digest(JAR)[..8]);
    let dir = data.join("agent");
    let path = dir.join(format!("riven-agent-{hash}.jar"));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    // A game starting from another launcher window may read it meanwhile: never a half-written jar.
    let partial = dir.join(format!("riven-agent-{hash}.jar.part"));
    std::fs::write(&partial, JAR).map_err(io(&partial))?;
    std::fs::rename(&partial, &path).map_err(io(&path))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_microsoft_join_never_goes_to_riven() {
        let dir = std::env::temp_dir().join(format!("riven-agent-test-{}", std::process::id()));
        let agent = SkinsAgent {
            api: "https://api.example.test/v1/skins".into(),
            key: "KEY".into(),
            game_token: Some("rgt1.x.y".into()),
        };
        let microsoft = agent.jvm_args(&dir, false).unwrap();
        assert!(
            !microsoft
                .iter()
                .any(|a| a.starts_with("-Driven.skins.join"))
        );
        let offline = agent.jvm_args(&dir, true).unwrap();
        assert!(offline.contains(&"-Driven.skins.join=true".to_owned()));
        let jar = offline[0].strip_prefix("-javaagent:").unwrap();
        assert_eq!(std::fs::read(jar).unwrap(), JAR);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
