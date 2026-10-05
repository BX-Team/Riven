use std::time::{Duration, SystemTime};

use rust_i18n::t;

/// "today", "3 d ago", "2 wk ago", "5 mo ago" for a file's modification time.
pub fn ago(time: SystemTime) -> String {
    let age = SystemTime::now()
        .duration_since(time)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let days = age / 86_400;
    match days {
        0 => t!("time.today").into(),
        1 => t!("time.yesterday").into(),
        2..=13 => t!("time.days", n = days).into(),
        14..=59 => t!("time.weeks", n = days / 7).into(),
        60..=729 => t!("time.months", n = days / 30).into(),
        _ => t!("time.years", n = days / 365).into(),
    }
}
