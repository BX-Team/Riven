use std::cmp::Ordering;
use std::fmt;

/// A loosely structured mod version (`0.8.13+mc1.21.1`, `21.1.77`, `1.0-beta.2`).
#[derive(Debug, Clone)]
pub struct ModVersion {
    raw: String,
    parts: Vec<Part>,
    /// Parts before the first `-`; Maven and semver rank them above the rest.
    main: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Num(u64),
    Text(String),
}

impl ModVersion {
    pub fn parse(raw: &str) -> Self {
        let core = raw.split('+').next().unwrap_or(raw).trim();
        let mut parts = Vec::new();
        let mut main = None;
        let mut current = String::new();
        let flush = |current: &mut String, parts: &mut Vec<Part>| {
            if current.is_empty() {
                return;
            }
            let part = match current.parse() {
                Ok(n) => Part::Num(n),
                Err(_) => Part::Text(current.to_ascii_lowercase()),
            };
            parts.push(part);
            current.clear();
        };
        for c in core.chars() {
            if matches!(c, '.' | '-' | '_') {
                flush(&mut current, &mut parts);
                if c == '-' && main.is_none() {
                    main = Some(parts.len());
                }
            } else {
                if current
                    .chars()
                    .last()
                    .is_some_and(|l| l.is_ascii_digit() != c.is_ascii_digit())
                {
                    flush(&mut current, &mut parts);
                }
                current.push(c);
            }
        }
        flush(&mut current, &mut parts);
        Self {
            raw: raw.to_owned(),
            main: main.unwrap_or(parts.len()),
            parts,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Leading numeric components (`1.21.1-pre2` → `[1, 21, 1]`).
    fn numeric_prefix(&self) -> Vec<u64> {
        self.parts
            .iter()
            .map_while(|p| match p {
                Part::Num(n) => Some(*n),
                Part::Text(_) => None,
            })
            .collect()
    }

    fn from_parts(parts: Vec<u64>) -> Self {
        let raw = parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".");
        Self {
            raw,
            main: parts.len(),
            parts: parts.into_iter().map(Part::Num).collect(),
        }
    }
}

/// Qualifier rank relative to a plain release (0).
fn qualifier(text: &str) -> i32 {
    match text {
        "dev" | "snapshot" => -6,
        "alpha" | "a" => -5,
        "beta" | "b" => -4,
        "milestone" | "m" => -3,
        "pre" | "preview" => -2,
        "rc" | "cr" => -1,
        "ga" | "final" | "release" => 0,
        _ => 1,
    }
}

fn compare_parts(a: Option<&Part>, b: Option<&Part>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (Some(Part::Num(x)), Some(Part::Num(y))) => x.cmp(y),
        (Some(Part::Num(x)), None) => x.cmp(&0),
        (None, Some(Part::Num(y))) => 0.cmp(y),
        (Some(Part::Text(x)), Some(Part::Text(y))) => {
            qualifier(x).cmp(&qualifier(y)).then_with(|| x.cmp(y))
        }
        (Some(Part::Text(x)), None) => qualifier(x).cmp(&0),
        (None, Some(Part::Text(y))) => 0.cmp(&qualifier(y)),
        (Some(Part::Num(_)), Some(Part::Text(_))) => Ordering::Greater,
        (Some(Part::Text(_)), Some(Part::Num(_))) => Ordering::Less,
    }
}

impl Ord for ModVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let compare = |a: &[Part], b: &[Part]| {
            (0..a.len().max(b.len()))
                .map(|i| compare_parts(a.get(i), b.get(i)))
                .find(|o| o.is_ne())
                .unwrap_or(Ordering::Equal)
        };
        let (main, rest) = self.parts.split_at(self.main);
        let (other_main, other_rest) = other.parts.split_at(other.main);
        compare(main, other_main).then_with(|| compare(rest, other_rest))
    }
}

impl PartialOrd for ModVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ModVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for ModVersion {}

impl fmt::Display for ModVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    version: ModVersion,
    inclusive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Interval {
    lower: Option<Bound>,
    upper: Option<Bound>,
}

impl Interval {
    fn exact(version: ModVersion) -> Self {
        let bound = Bound {
            version,
            inclusive: true,
        };
        Self {
            lower: Some(bound.clone()),
            upper: Some(bound),
        }
    }

    fn contains(&self, v: &ModVersion) -> bool {
        let above = self.lower.as_ref().is_none_or(|b| match v.cmp(&b.version) {
            Ordering::Greater => true,
            Ordering::Equal => b.inclusive,
            Ordering::Less => false,
        });
        let below = self.upper.as_ref().is_none_or(|b| match v.cmp(&b.version) {
            Ordering::Less => true,
            Ordering::Equal => b.inclusive,
            Ordering::Greater => false,
        });
        above && below
    }

    fn intersect(mut self, other: Interval) -> Self {
        if let Some(lower) = other.lower
            && self.lower.as_ref().is_none_or(|l| {
                lower.version > l.version || (lower.version == l.version && !lower.inclusive)
            })
        {
            self.lower = Some(lower);
        }
        if let Some(upper) = other.upper
            && self.upper.as_ref().is_none_or(|u| {
                upper.version < u.version || (upper.version == u.version && !upper.inclusive)
            })
        {
            self.upper = Some(upper);
        }
        self
    }
}

/// A set of acceptable versions: a union of intervals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    raw: String,
    any_of: Vec<Interval>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid version range `{0}`")]
pub struct RangeError(pub String);

impl VersionReq {
    pub fn any() -> Self {
        Self {
            raw: "*".into(),
            any_of: vec![Interval::default()],
        }
    }

    pub fn matches(&self, version: &ModVersion) -> bool {
        self.any_of.iter().any(|i| i.contains(version))
    }

    pub fn is_any(&self) -> bool {
        self.any_of
            .iter()
            .any(|i| i.lower.is_none() && i.upper.is_none())
    }

    /// Fabric predicates; several alternatives (a JSON array) are OR-ed.
    pub fn fabric<S: AsRef<str>>(alternatives: &[S]) -> Result<Self, RangeError> {
        let raw = alternatives
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join(" || ");
        let mut any_of = Vec::new();
        for alt in alternatives {
            for alt in alt.as_ref().split("||") {
                any_of.push(fabric_conjunction(alt).ok_or_else(|| RangeError(raw.clone()))?);
            }
        }
        if any_of.is_empty() {
            any_of.push(Interval::default());
        }
        Ok(Self { raw, any_of })
    }

    /// Maven ranges (`[1.0,2.0)`, `[21.1,)`); a bare version is only a recommendation.
    pub fn maven(range: &str) -> Result<Self, RangeError> {
        let raw = range.trim();
        let err = || RangeError(raw.to_owned());
        if raw.is_empty() || raw == "*" || !raw.starts_with(['[', '(']) {
            return Ok(Self {
                raw: if raw.is_empty() { "*" } else { raw }.to_owned(),
                ..Self::any()
            });
        }
        let mut any_of = Vec::new();
        let mut rest = raw;
        while !rest.is_empty() {
            let open = rest.chars().next().ok_or_else(err)?;
            let end = rest.find([']', ')']).ok_or_else(err)?;
            let close = rest[end..].chars().next().ok_or_else(err)?;
            let body = &rest[1..end];
            let bound = |s: &str, inclusive| {
                let s = s.trim();
                (!s.is_empty()).then(|| Bound {
                    version: ModVersion::parse(s),
                    inclusive,
                })
            };
            let interval = match body.split_once(',') {
                Some((lo, hi)) => Interval {
                    lower: bound(lo, open == '['),
                    upper: bound(hi, close == ']'),
                },
                None if open == '[' && close == ']' => Interval::exact(ModVersion::parse(body)),
                None => return Err(err()),
            };
            any_of.push(interval);
            rest = rest[end + 1..].trim_start_matches([',', ' ']);
        }
        Ok(Self {
            raw: raw.to_owned(),
            any_of,
        })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn fabric_conjunction(predicate: &str) -> Option<Interval> {
    let mut interval = Interval::default();
    for term in predicate.split_whitespace() {
        interval = interval.intersect(fabric_term(term)?);
    }
    Some(interval)
}

fn fabric_term(term: &str) -> Option<Interval> {
    if term == "*" {
        return Some(Interval::default());
    }
    let (op, version) = ["<=", ">=", "<", ">", "=", "~", "^"]
        .iter()
        .find_map(|op| term.strip_prefix(op).map(|v| (*op, v)))
        .unwrap_or(("", term));
    if version.is_empty() {
        return None;
    }
    let lower = |inclusive| Bound {
        version: ModVersion::parse(version),
        inclusive,
    };
    let wildcard = version
        .split('.')
        .position(|p| matches!(p, "x" | "X" | "*"));
    if let Some(at) = wildcard {
        if !matches!(op, "" | "=") {
            return None;
        }
        let fixed = ModVersion::parse(&version.split('.').take(at).collect::<Vec<_>>().join("."))
            .numeric_prefix();
        return Some(prefix_interval(fixed));
    }
    Some(match op {
        "" | "=" => Interval::exact(ModVersion::parse(version)),
        ">=" => Interval {
            lower: Some(lower(true)),
            upper: None,
        },
        ">" => Interval {
            lower: Some(lower(false)),
            upper: None,
        },
        "<=" => Interval {
            lower: None,
            upper: Some(lower(true)),
        },
        "<" => Interval {
            lower: None,
            upper: Some(lower(false)),
        },
        "~" => {
            let nums = ModVersion::parse(version).numeric_prefix();
            let keep = if nums.len() >= 2 { 2 } else { 1 };
            Interval {
                lower: Some(lower(true)),
                ..prefix_interval(nums.into_iter().take(keep).collect())
            }
        }
        "^" => {
            let nums = ModVersion::parse(version).numeric_prefix();
            Interval {
                lower: Some(lower(true)),
                ..prefix_interval(nums.into_iter().take(1).collect())
            }
        }
        _ => return None,
    })
}

/// Versions starting with `prefix` (`[1, 20]` → `>=1.20 <1.21`).
fn prefix_interval(prefix: Vec<u64>) -> Interval {
    let Some((last, head)) = prefix.split_last() else {
        return Interval::default();
    };
    let mut next = head.to_vec();
    next.push(last + 1);
    Interval {
        lower: Some(Bound {
            version: ModVersion::from_parts(prefix.clone()),
            inclusive: true,
        }),
        upper: Some(Bound {
            version: ModVersion::from_parts(next),
            inclusive: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> ModVersion {
        ModVersion::parse(s)
    }

    #[test]
    fn orders_real_world_versions() {
        assert!(v("0.8.13+mc1.21.1") > v("0.8.2+mc1.21.1"));
        assert!(v("21.1.77") > v("21.1.9"));
        assert!(v("1.0.0-beta.2") < v("1.0.0"));
        assert!(v("1.0.0-beta.2") > v("1.0.0-alpha.9"));
        assert!(v("1.0.0-rc.1") < v("1.0.0"));
        assert!(v("1.21.1-pre2") < v("1.21.1"));
        assert_eq!(v("1.21"), v("1.21.0"));
        assert_eq!(v("1.0+build.5"), v("1.0+build.9"));
        assert!(v("0.16.10") > v("0.16.9"));
        assert!(v("1.21.1-3.0.17") > v("1.21-2.29.0"));
        assert!(v("mc1.21.1-0.8.13") > v("mc1.21.1-0.8.2"));
        assert!(v("21.0.0-beta") < v("21.0.0"));
    }

    #[test]
    fn fabric_predicates() {
        let req = |s: &str| VersionReq::fabric(&[s]).unwrap();
        assert!(req(">=0.15.0").matches(&v("0.16.10")));
        assert!(!req(">=0.15.0").matches(&v("0.14.24")));
        assert!(req("~1.21").matches(&v("1.21.1")));
        assert!(!req("~1.21").matches(&v("1.22")));
        assert!(req("~1.2.3").matches(&v("1.2.9")));
        assert!(!req("~1.2.3").matches(&v("1.3.0")));
        assert!(req("^1.2.3").matches(&v("1.9.0")));
        assert!(!req("^1.2.3").matches(&v("2.0.0")));
        assert!(req("1.21.x").matches(&v("1.21.4")));
        assert!(!req("1.21.x").matches(&v("1.22")));
        assert!(req(">=1.20 <1.21").matches(&v("1.20.6")));
        assert!(!req(">=1.20 <1.21").matches(&v("1.21")));
        assert!(req("*").is_any());
        assert!(req("1.21.1").matches(&v("1.21.1")));
        assert!(!req("1.21.1").matches(&v("1.21.2")));

        let alts = VersionReq::fabric(&["1.20.1", ">=1.21"]).unwrap();
        assert!(alts.matches(&v("1.20.1")) && alts.matches(&v("1.21.4")));
        assert!(!alts.matches(&v("1.20.4")));
        assert!(VersionReq::fabric(&[">="]).is_err());
    }

    #[test]
    fn maven_ranges() {
        let req = |s: &str| VersionReq::maven(s).unwrap();
        assert!(req("[21.1,)").matches(&v("21.1.77")));
        assert!(req("[1.21-2.29.0,)").matches(&v("1.21.1-3.0.17")));
        assert!(!req("[21.1,)").matches(&v("21.0.167")));
        assert!(req("[1.21.1,1.22)").matches(&v("1.21.1")));
        assert!(!req("[1.21.1,1.22)").matches(&v("1.22")));
        assert!(req("(,1.0]").matches(&v("0.9")));
        assert!(req("[1.0]").matches(&v("1.0")) && !req("[1.0]").matches(&v("1.0.1")));
        assert!(req("[1,2),[3,4)").matches(&v("3.5")));
        assert!(!req("[1,2),[3,4)").matches(&v("2.5")));
        assert!(req("1.0").is_any() && req("*").is_any() && req("").is_any());
        assert!(VersionReq::maven("[1.0").is_err());
    }
}
