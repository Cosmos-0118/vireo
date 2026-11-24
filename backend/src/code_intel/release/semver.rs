//! Minimal SemVer value used by the release engine (parse, order, bump).

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Ver {
    /// Parses `1.2.3`, `v1.2.3`, `1.2.3-rc.1` (build metadata after `+` is dropped).
    pub fn parse(s: &str) -> Option<Ver> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next().unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            _ => (s, None),
        };
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() || pre.as_deref().is_some_and(|p| !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
            return None;
        }
        Some(Ver { major, minor, patch, pre })
    }

    pub fn core(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    pub fn is_pre(&self) -> bool {
        self.pre.is_some()
    }

    fn stable(&self) -> Ver {
        Ver { pre: None, ..self.clone() }
    }

    /// Resolves `patch|minor|major|prerelease` against `self`, or parses an explicit version.
    pub fn resolve(&self, arg: &str) -> Result<Ver, String> {
        let arg = arg.trim();
        let (maj, min, pat) = (self.major, self.minor, self.patch);
        let v = |major, minor, patch, pre| Ver { major, minor, patch, pre };
        Ok(match arg.to_ascii_lowercase().as_str() {
            // Leaving a prerelease (0.5.0-rc.1 -> 0.5.0) is the "next" release of that line.
            "patch" if self.is_pre() => self.stable(),
            "patch" => v(maj, min, pat + 1, None),
            "minor" if self.is_pre() && pat == 0 => self.stable(),
            "minor" => v(maj, min + 1, 0, None),
            "major" if self.is_pre() && min == 0 && pat == 0 => self.stable(),
            "major" => v(maj + 1, 0, 0, None),
            "prerelease" | "pre" | "rc" => match &self.pre {
                Some(p) => {
                    let mut parts: Vec<String> = p.split('.').map(str::to_string).collect();
                    match parts.last().and_then(|l| l.parse::<u64>().ok()) {
                        Some(n) => *parts.last_mut().unwrap() = (n + 1).to_string(),
                        None => parts.push("1".into()),
                    }
                    v(maj, min, pat, Some(parts.join(".")))
                }
                None => v(maj, min, pat + 1, Some("rc.1".into())),
            },
            _ => Ver::parse(arg).ok_or_else(|| {
                format!("`{arg}` is not patch|minor|major|prerelease or a version like 1.2.3 / 1.2.3-rc.1")
            })?,
        })
    }
}

impl fmt::Display for Ver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl Ord for Ver {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(o.major, o.minor, o.patch))
            .then_with(|| match (&self.pre, &o.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

impl PartialOrd for Ver {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// SemVer §11: dot-separated identifiers, numeric ones compare as numbers and rank below text.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}
