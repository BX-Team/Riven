use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::forge::{InstallProfile, Processor, entry};
use crate::{Dirs, Error, Event, Prepared, Report, Step, io, maven, net, os};

/// Runs the installer's client processors once per installer: they patch the game jar into the libraries.
pub async fn run(
    dirs: &Dirs,
    prepared: &Prepared,
    installer: &Path,
    profile: &InstallProfile,
    java: &Path,
    report: Report<'_>,
) -> Result<(), Error> {
    let steps: Vec<&Processor> = profile
        .processors
        .iter()
        .filter(|p| {
            p.sides
                .as_ref()
                .is_none_or(|s| s.iter().any(|s| s == "client"))
        })
        .collect();
    if steps.is_empty() {
        return Ok(());
    }
    let marker = installer.with_extension("processed");
    let stamp = {
        let jar = installer.to_owned();
        crate::blocking(move || net::sha1_file(&jar).map_err(io(&jar))).await?
    };
    if std::fs::read_to_string(&marker).is_ok_and(|s| s.trim() == stamp) {
        return Ok(());
    }
    report(Event::Step(Step::Processors));
    let scratch = dirs
        .scratch()
        .join(installer.file_stem().unwrap_or_default());
    let _ = std::fs::remove_dir_all(&scratch);
    let data = {
        let (dirs, installer, scratch) = (dirs.clone(), installer.to_owned(), scratch.clone());
        let (data, minecraft) = (profile.data.clone(), prepared.minecraft.clone());
        crate::blocking(move || variables(&dirs, &installer, &scratch, &data, &minecraft)).await?
    };
    let total = steps.len() as u64;
    for (done, step) in steps.into_iter().enumerate() {
        report(Event::Amount {
            done: done as u64,
            total,
        });
        execute(dirs, step, &data, java, &scratch).await?;
    }
    report(Event::Amount { done: total, total });
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::write(&marker, stamp).map_err(io(&marker))
}

/// `{NAME}` values the processors' arguments refer to, for the client side.
fn variables(
    dirs: &Dirs,
    installer: &Path,
    scratch: &Path,
    data: &BTreeMap<String, crate::forge::Sided>,
    minecraft: &str,
) -> Result<BTreeMap<String, String>, Error> {
    let libraries = dirs.libraries();
    let display = |p: &Path| p.display().to_string();
    let client = dirs
        .versions()
        .join(minecraft)
        .join(format!("{minecraft}.jar"));
    let mut vars = BTreeMap::from([
        ("SIDE".to_owned(), "client".to_owned()),
        ("MINECRAFT_VERSION".to_owned(), minecraft.to_owned()),
        ("MINECRAFT_JAR".to_owned(), display(&client)),
        ("ROOT".to_owned(), display(dirs.root())),
        ("INSTALLER".to_owned(), display(installer)),
        ("LIBRARY_DIR".to_owned(), display(&libraries)),
    ]);
    let mut archive = None;
    for (key, value) in data {
        let value = &value.client;
        let resolved = if let Some(coord) =
            value.strip_prefix('[').and_then(|v| v.strip_suffix(']'))
        {
            display(&libraries.join(maven::path(coord).unwrap_or_default()))
        } else if let Some(literal) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            literal.to_owned()
        } else if let Some(inner) = value.strip_prefix('/') {
            let archive = match &mut archive {
                Some(a) => a,
                None => archive.insert(open(installer)?),
            };
            let target = scratch.join(inner);
            net::save(&target, &entry(archive, installer, inner)?)?;
            display(&target)
        } else {
            value.clone()
        };
        vars.insert(key.clone(), resolved);
    }
    Ok(vars)
}

fn open(jar: &Path) -> Result<zip::ZipArchive<std::fs::File>, Error> {
    let file = std::fs::File::open(jar).map_err(io(jar))?;
    zip::ZipArchive::new(file).map_err(|source| Error::Zip {
        path: jar.to_owned(),
        source,
    })
}

/// Fills `{NAME}` placeholders; a whole `[coordinate]` becomes the library's path.
fn substitute(arg: &str, vars: &BTreeMap<String, String>, libraries: &Path) -> String {
    if let Some(coord) = arg.strip_prefix('[').and_then(|a| a.strip_suffix(']')) {
        return libraries
            .join(maven::path(coord).unwrap_or_default())
            .display()
            .to_string();
    }
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let key = &rest[start + 1..start + len];
        out.push_str(&rest[..start]);
        match vars.get(key) {
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[start..=start + len]),
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

async fn execute(
    dirs: &Dirs,
    step: &Processor,
    vars: &BTreeMap<String, String>,
    java: &Path,
    scratch: &Path,
) -> Result<(), Error> {
    let libraries = dirs.libraries();
    let outputs: Vec<(PathBuf, String)> = step
        .outputs
        .iter()
        .map(|(path, sha1)| {
            (
                PathBuf::from(substitute(path, vars, &libraries)),
                substitute(sha1, vars, &libraries),
            )
        })
        .collect();
    if !outputs.is_empty() && outputs_match(&outputs).await? {
        return Ok(());
    }
    let lib = |name: &str| libraries.join(maven::path(name).unwrap_or_default());
    let jar = lib(&step.jar);
    let main_class = {
        let jar = jar.clone();
        crate::blocking(move || main_class(&jar)).await?
    };
    let classpath: Vec<String> = std::iter::once(&step.jar)
        .chain(&step.classpath)
        .map(|name| lib(name).display().to_string())
        .collect();
    let args: Vec<String> = step
        .args
        .iter()
        .map(|a| substitute(a, vars, &libraries))
        .collect();
    std::fs::create_dir_all(scratch).map_err(io(scratch))?;
    let mut command = tokio::process::Command::new(java);
    command
        .arg("-cp")
        .arg(classpath.join(os::CLASSPATH_SEPARATOR))
        .arg(&main_class)
        .args(&args)
        .current_dir(scratch)
        .stdin(std::process::Stdio::null());
    let output = command.output().await.map_err(|source| Error::Spawn {
        program: java.to_owned(),
        source,
    })?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        let text = if text.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout)
        } else {
            text
        };
        let tail: Vec<&str> = text.lines().rev().take(8).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(Error::Processor(format!(
            "{} exited with {}: {}",
            step.jar,
            output.status,
            tail.join("\n")
        )));
    }
    if !outputs.is_empty() && !outputs_match(&outputs).await? {
        return Err(Error::Processor(format!(
            "{} produced files with the wrong checksum",
            step.jar
        )));
    }
    Ok(())
}

async fn outputs_match(outputs: &[(PathBuf, String)]) -> Result<bool, Error> {
    let outputs = outputs.to_vec();
    crate::blocking(move || {
        Ok(outputs.iter().all(|(path, sha1)| {
            net::sha1_file(path).is_ok_and(|h| h.eq_ignore_ascii_case(sha1.trim_matches('\'')))
        }))
    })
    .await
}

/// `Main-Class` from a jar's manifest.
fn main_class(jar: &Path) -> Result<String, Error> {
    let mut archive = open(jar)?;
    let manifest = entry(&mut archive, jar, "META-INF/MANIFEST.MF")?;
    String::from_utf8_lossy(&manifest)
        .lines()
        .find_map(|l| l.strip_prefix("Main-Class:"))
        .map(|c| c.trim().to_owned())
        .ok_or_else(|| Error::Processor(format!("{} names no main class", jar.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_take_data_values_and_library_paths() {
        let vars = BTreeMap::from([
            ("SIDE".to_owned(), "client".to_owned()),
            ("ROOT".to_owned(), "/data".to_owned()),
        ]);
        let libs = Path::new("/libs");
        assert_eq!(substitute("{SIDE}", &vars, libs), "client");
        assert_eq!(
            substitute("{ROOT}/libraries/", &vars, libs),
            "/data/libraries/"
        );
        assert_eq!(substitute("{UNKNOWN}", &vars, libs), "{UNKNOWN}");
        assert_eq!(
            substitute(
                "[net.neoforged:neoform:1.21.1-20240808.144430@zip]",
                &vars,
                libs
            ),
            libs.join(
                "net/neoforged/neoform/1.21.1-20240808.144430/neoform-1.21.1-20240808.144430.zip"
            )
            .display()
            .to_string()
        );
    }
}
