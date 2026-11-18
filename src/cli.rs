//! Review options, with the rest of the command line passed to Rune.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

use crate::source::Repository;

pub(crate) const USAGE: &str =
    "Usage: layeredgit [--repo PATH] [--base REF] [--demo] [--demo-pulse] [Rune options]";

pub(crate) struct Options {
    pub(crate) launch: Launch,
    pub(crate) demo_pulse: bool,
    pub(crate) preload: Vec<PathBuf>,
    /// Rune's flags, in the order the user supplied them.
    pub(crate) tooling: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    Start,
    Review(Repository),
    Demo,
}

impl Options {
    pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut args = args.into_iter().peekable();
        let mut paths = Vec::new();
        let mut base = None;
        let mut demo = false;
        let mut demo_pulse = false;
        let mut tooling = Vec::new();
        while let Some(arg) = args.next() {
            let mut parts = arg.as_bytes().splitn(2, |&byte| byte == b'=');
            let flag = OsStr::from_bytes(parts.next().unwrap_or_default());
            let inline = parts.next().map(OsStr::from_bytes);
            match flag.as_bytes() {
                b"--repo" | b"--base" => {
                    let value = inline
                        .map(OsStr::to_owned)
                        .or_else(|| args.next_if(|next| !next.as_bytes().starts_with(b"--")))
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| format!("{} needs a value", flag.to_string_lossy()))?;
                    if flag == "--repo" {
                        paths.push(PathBuf::from(value));
                    } else {
                        base = Some(value);
                    }
                }
                b"--demo" if inline.is_none() => demo = true,
                b"--demo-pulse" if inline.is_none() => demo_pulse = true,
                flag if flag.starts_with(b"-psn_") => {}
                _ => {
                    tooling.push(arg.into_string().map_err(|arg| {
                        format!("Rune arguments must be UTF-8: {}", arg.display())
                    })?);
                }
            }
        }
        let path = paths.pop();
        let launch = if demo || demo_pulse && path.is_none() && base.is_none() {
            Launch::Demo
        } else if path.is_some() || base.is_some() {
            Launch::Review(Repository {
                path: path.unwrap_or_else(|| PathBuf::from(".")),
                base,
            })
        } else {
            Launch::Start
        };
        Ok(Self {
            launch,
            demo_pulse,
            preload: paths,
            tooling,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, reason = "The supplied options are valid")]
    fn separates_interleaved_options_and_preserves_rune_argument_order() {
        let options = Options::parse(
            [
                "--size",
                "1400x900",
                "--repo",
                "a path",
                "--tree",
                "out.json",
                "--base=refs/heads/main",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(
            options.launch,
            Launch::Review(Repository {
                path: "a path".into(),
                base: Some("refs/heads/main".into()),
            })
        );
        assert_eq!(
            options.tooling,
            ["--size", "1400x900", "--tree", "out.json"]
        );
        assert!(Options::parse(["--repo", "--snap", "out.png"].map(OsString::from)).is_err());
        assert_eq!(
            Options::parse(["--demo", "--snap", "out.png"].map(OsString::from))
                .unwrap()
                .launch,
            Launch::Demo,
        );
    }

    #[test]
    #[allow(clippy::unwrap_used, reason = "The supplied options are valid")]
    fn distinguishes_finder_launches_and_base_only_terminal_launches() {
        assert_eq!(Options::parse([]).unwrap().launch, Launch::Start);
        assert_eq!(
            Options::parse([OsString::from("-psn_0_42")])
                .unwrap()
                .launch,
            Launch::Start
        );
        assert_eq!(
            Options::parse([OsString::from("--base=-previous")])
                .unwrap()
                .launch,
            Launch::Review(Repository {
                path: ".".into(),
                base: Some("-previous".into())
            })
        );
    }
}
