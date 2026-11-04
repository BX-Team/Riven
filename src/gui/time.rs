use std::time::{Duration, SystemTime};

use rust_i18n::t;

/// "just now", "5 min ago", "3 h ago", then like [`ago`]: for things made today that need telling apart.
pub fn since(time: SystemTime) -> String {
    let age = SystemTime::now()
        .duration_since(time)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match age {
        0..60 => t!("time.just_now").into(),
        60..3_600 => t!("time.minutes_ago", n = age / 60).into(),
        3_600..86_400 => t!("time.hours_ago", n = age / 3_600).into(),
        _ => ago(time),
    }
}

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
