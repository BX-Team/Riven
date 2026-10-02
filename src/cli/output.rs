use std::time::Duration;

use console::{Term, style};
use indicatif::{ProgressBar, ProgressStyle};
use riven_format::{Entry, Side};
use riven_resolve::Plan;
use serde_json::{Value, json};

/// Human output on stderr/stdout, or one JSON document on stdout with `--json`.
pub struct Output {
    json: bool,
    interactive: bool,
}

impl Output {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            interactive: !json && Term::stderr().is_term(),
        }
    }

    pub fn spinner(&self, message: impl Into<String>) -> ProgressBar {
        if !self.interactive {
            return ProgressBar::hidden();
        }
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("valid template"),
        );
        bar.set_message(message.into());
        bar.enable_steady_tick(Duration::from_millis(100));
        bar
    }

    /// Prints `value` in JSON mode; otherwise runs `human`.
    pub fn emit(&self, value: Value, human: impl FnOnce()) {
        if self.json {
            println!("{value:#}");
        } else {
            human();
        }
    }

    pub fn success(&self, message: &str) {
        if !self.json {
            eprintln!("{} {message}", style("✓").green().bold());
        }
    }

    pub fn warn(&self, message: &str) {
        if !self.json {
            eprintln!("{} {message}", style("warning:").yellow().bold());
        }
    }

    pub fn note(&self, message: &str) {
        if !self.json {
            eprintln!("{} {message}", style("note:").cyan());
        }
    }

    pub fn error(&self, error: &anyhow::Error) {
        if self.json {
            println!("{:#}", json!({ "error": format!("{error:#}") }));
        } else {
            eprintln!("{} {error:#}", style("error:").red().bold());
        }
    }

    /// Prints a plan; `problems` and `notes` go to stderr as warnings.
    pub fn plan(&self, plan: &Plan) {
        if self.json {
            println!("{:#}", plan_json(plan));
            return;
        }
        for entry in &plan.add {
            let via = match entry.reason {
                riven_format::Reason::Dependency => style(" (dependency)").dim().to_string(),
                riven_format::Reason::Explicit => String::new(),
            };
            println!(
                "{} {} {}{}{}",
                style("+").green().bold(),
                style(&entry.id).bold(),
                file_name(entry),
                side_label(entry.side),
                via
            );
        }
        for (old, new) in &plan.update {
            println!(
                "{} {} {} → {}",
                style("~").yellow().bold(),
                style(&new.id).bold(),
                style(file_name(old)).dim(),
                file_name(new)
            );
        }
        for entry in &plan.remove {
            println!(
                "{} {} {}",
                style("-").red().bold(),
                style(&entry.id).bold(),
                style(file_name(entry)).dim()
            );
        }
        for problem in &plan.problems {
            self.warn(&problem.to_string());
        }
        for note in &plan.notes {
            self.note(note);
        }
    }
}

pub fn file_name(entry: &Entry) -> &str {
    entry.file.path.file_name()
}

pub fn side_label(side: Side) -> String {
    match side {
        Side::Both => String::new(),
        Side::Client => style(" [client]").cyan().to_string(),
        Side::Server => style(" [server]").magenta().to_string(),
    }
}

pub fn plan_json(plan: &Plan) -> Value {
    json!({
        "add": plan.add,
        "update": plan.update.iter().map(|(from, to)| json!({ "from": from, "to": to })).collect::<Vec<_>>(),
        "remove": plan.remove,
        "problems": plan.problems.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "notes": plan.notes,
    })
}
