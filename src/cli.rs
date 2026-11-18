//! Review options, with the rest of the command line passed to Rune.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

use crate::source::Repository;

pub(crate) const USAGE: &str =
    "Usage: git-viewer-rune [--repo PATH] [--base REF] [--demo] [--demo-pulse] [Rune options]";

pub(crate) struct Options {
    /// Absent in demo mode.
    pub(crate) repository: Option<Repository>,
    pub(crate) demo_pulse: bool,
    /// Rune's flags, in the order the user supplied them.
    pub(crate) tooling: Vec<String>,
}

impl Options {
    pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut args = args.into_iter().peekable();
        let mut repository = Repository {
            path: PathBuf::from("."),
            base: None,
        };
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
                        repository.path = value.into();
                    } else {
                        repository.base = Some(value);
                    }
                }
                b"--demo" if inline.is_none() => demo = true,
                b"--demo-pulse" if inline.is_none() => demo_pulse = true,
                _ => {
                    tooling.push(arg.into_string().map_err(|arg| {
                        format!("Rune arguments must be UTF-8: {}", arg.display())
                    })?);
                }
            }
        }
        Ok(Self {
            repository: (!demo).then_some(repository),
            demo_pulse,
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
        let repository = options.repository.unwrap();
        assert_eq!(repository.path, PathBuf::from("a path"));
        assert_eq!(
            repository.base.as_deref(),
            Some(OsStr::new("refs/heads/main"))
        );
        assert_eq!(
            options.tooling,
            ["--size", "1400x900", "--tree", "out.json"]
        );
        assert!(Options::parse(["--repo", "--snap", "out.png"].map(OsString::from)).is_err());
        assert!(
            Options::parse(["--demo", "--snap", "out.png"].map(OsString::from))
                .unwrap()
                .repository
                .is_none()
        );
    }
}
