use clap::CommandFactory as _;
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use riven_build::workspace::Workspace;

use super::Cli;

/// Answers the shell when it asks for completions (`COMPLETE=<shell>`); returns otherwise.
pub fn answer() {
    // A bare name, so the registration finds `riven` on PATH, `riven.com` on Windows included.
    clap_complete::CompleteEnv::with_factory(Cli::command)
        .bin("riven")
        .completer("riven")
        .complete();
}

/// Ids of the entries in the `riven.json` around the current directory.
pub fn entries() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        Workspace::find()
            .map(|ws| {
                ws.project
                    .content
                    .into_iter()
                    .map(|e| CompletionCandidate::new(e.id).help(Some(e.name.into())))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn group_candidates() -> Vec<CompletionCandidate> {
    Workspace::find()
        .map(|ws| {
            ws.project
                .groups
                .into_iter()
                .map(|g| CompletionCandidate::new(g.id).help(Some(g.name.into())))
                .collect()
        })
        .unwrap_or_default()
}

/// Ids of the pack's optional groups.
pub fn groups() -> ArgValueCandidates {
    ArgValueCandidates::new(group_candidates)
}

/// The groups, and `none` to take an entry out of its group.
pub fn groups_or_none() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let mut all = group_candidates();
        all.push(CompletionCandidate::new("none").help(Some("no group".into())));
        all
    })
}
