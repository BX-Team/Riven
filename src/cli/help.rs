use std::fmt::Write as _;

use clap::CommandFactory as _;
use clap::builder::StyledStr;

use super::Cli;

/// Subcommands of `riven --help` by what they work on, in the order they are listed.
const SECTIONS: &[(&str, &[&str])] = &[
    ("Pack", &["init", "import", "migrate", "bump", "check"]),
    (
        "Content",
        &[
            "search", "add", "remove", "update", "list", "why", "pin", "unpin", "side", "group",
        ],
    ),
    (
        "Releases",
        &["keygen", "build", "publish", "deploy", "export"],
    ),
    ("Installing", &["install", "status", "trust", "gc"]),
    ("Riven", &["self-update"]),
];

/// The clap command with its subcommands listed by section instead of one flat list.
pub fn command() -> clap::Command {
    let cmd = Cli::command();
    let template = template(&cmd);
    cmd.help_template(template)
}

fn template(cmd: &clap::Command) -> StyledStr {
    let styles = cmd.get_styles();
    let header = styles.get_header();
    let literal = styles.get_literal();
    let width = SECTIONS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|name| name.len())
        .max()
        .unwrap_or(0);

    let mut out = String::from("{about-with-newline}\n{usage-heading} {usage}\n");
    for (title, names) in SECTIONS {
        let _ = write!(out, "\n{header}{title}:{header:#}\n");
        for name in *names {
            let about = cmd
                .find_subcommand(name)
                .and_then(|sub| sub.get_about())
                .map(|about| about.to_string())
                .unwrap_or_default();
            let pad = width - name.len();
            let _ = writeln!(out, "  {literal}{name}{literal:#}{:pad$}  {about}", "");
        }
    }
    let _ = write!(
        out,
        "\n{header}Options:{header:#}\n{{options}}{{after-help}}"
    );
    StyledStr::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_visible_subcommand_is_in_exactly_one_section() {
        let cmd = command();
        let listed: Vec<&str> = SECTIONS
            .iter()
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
            let name = sub.get_name();
            let times = listed.iter().filter(|n| **n == name).count();
            assert_eq!(times, 1, "`{name}` is listed {times} times in SECTIONS");
        }
        for name in listed {
            assert!(
                cmd.find_subcommand(name).is_some(),
                "SECTIONS names `{name}`, which is not a subcommand"
            );
        }
    }
}
