/// A Maven coordinate: `group:artifact:version[:classifier][@extension]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coord<'a> {
    pub group: &'a str,
    pub artifact: &'a str,
    pub version: &'a str,
    pub classifier: Option<&'a str>,
    pub extension: &'a str,
}

impl<'a> Coord<'a> {
    pub fn parse(name: &'a str) -> Option<Self> {
        let (name, extension) = name.split_once('@').unwrap_or((name, "jar"));
        let mut parts = name.split(':');
        let group = parts.next().filter(|s| !s.is_empty())?;
        let artifact = parts.next().filter(|s| !s.is_empty())?;
        let version = parts.next().filter(|s| !s.is_empty())?;
        let classifier = parts.next();
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            group,
            artifact,
            version,
            classifier,
            extension,
        })
    }

    /// The path under a repository or the libraries folder, always with `/`.
    pub fn path(&self) -> String {
        let file = match self.classifier {
            Some(c) => format!("{}-{}-{c}.{}", self.artifact, self.version, self.extension),
            None => format!("{}-{}.{}", self.artifact, self.version, self.extension),
        };
        format!(
            "{}/{}/{}/{file}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version
        )
    }

    /// `group:artifact[:classifier]`: two libraries with the same key are one library.
    pub fn key(&self) -> String {
        match self.classifier {
            Some(c) => format!("{}:{}:{c}", self.group, self.artifact),
            None => format!("{}:{}", self.group, self.artifact),
        }
    }
}

/// `name`'s path under the libraries folder; names that are not coordinates give none.
pub fn path(name: &str) -> Option<String> {
    Coord::parse(name).map(|c| c.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_keep_classifiers_and_extensions() {
        assert_eq!(
            path("org.lwjgl:lwjgl:3.3.3:natives-linux").as_deref(),
            Some("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar")
        );
        assert_eq!(
            path("de.oceanlabs.mcp:mcp_config:1.20.1-20230612.114412:mappings@txt").as_deref(),
            Some(
                "de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412-mappings.txt"
            )
        );
        assert_eq!(
            path("net.neoforged:neoform:1.21.1-20240808.144430@zip").as_deref(),
            Some("net/neoforged/neoform/1.21.1-20240808.144430/neoform-1.21.1-20240808.144430.zip")
        );
        assert_eq!(path("not-a-coordinate"), None);
    }
}
