//! Cache only ignored directory prefixes that cannot contain a tracked path.

use std::{
    collections::HashSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use repo::Repo;

// Git config can include files outside the watched directories. Re-resolve the
// path at most once per fallback-poll interval while notifications are flowing.
const CONFIG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Ignore {
    directories: HashSet<PathBuf>,
    config_checked: Option<Instant>,
    excludes: Option<PathBuf>,
    excludes_stamp: Option<(u64, i64, i64, u64)>,
}

impl Ignore {
    pub(super) fn update_rules(&mut self, repo: &Repo, reset: bool, read_config: bool) -> bool {
        let before = self.excludes.clone();
        let initialized = self.config_checked.is_some();
        if read_config
            || self
                .config_checked
                .is_none_or(|last| last.elapsed() >= CONFIG_INTERVAL)
        {
            self.config_checked = Some(Instant::now());
            let Ok(path) = repo.excludes_file() else {
                self.directories.clear();
                self.config_checked = None;
                return true;
            };
            self.excludes = path;
        }
        let stamp = self
            .excludes
            .as_deref()
            .and_then(|path| fs::metadata(path).ok())
            .map(|meta| (meta.ino(), meta.ctime(), meta.ctime_nsec(), meta.len()));
        let changed = initialized && (before != self.excludes || self.excludes_stamp != stamp);
        self.excludes_stamp = stamp;
        if reset || read_config || changed {
            self.directories.clear();
        }
        changed
    }

    pub(super) fn affects_review(&mut self, repo: &Repo, mut paths: HashSet<PathBuf>) -> bool {
        // An ancestor notification may mean a directory was moved or replaced.
        // Check the prefix still names a directory before ignoring events at it.
        self.directories.retain(|dir| {
            !dir.ancestors().skip(1).any(|parent| paths.contains(parent)) && is_directory(repo, dir)
        });
        paths.retain(|path| {
            !path
                .ancestors()
                .any(|ancestor| self.directories.contains(ancestor))
        });
        if paths.is_empty() {
            return false;
        }

        // Some backends report only the file, so also ask Git about its parent
        // directories. A file matching *.log alone must never seed a prefix.
        let mut candidates = HashSet::new();
        for path in &paths {
            candidates.extend(
                path.ancestors()
                    .take_while(|part| !part.as_os_str().is_empty())
                    .filter(|part| is_directory(repo, part)),
            );
        }
        let Ok(ignored) = repo.ignored_paths(
            paths.iter().map(PathBuf::as_path).chain(
                candidates
                    .iter()
                    .copied()
                    .filter(|dir| !paths.contains(*dir)),
            ),
        ) else {
            return true;
        };
        let ignored_dirs: Vec<_> = candidates
            .into_iter()
            .filter(|dir| ignored.contains(*dir))
            .collect();
        if !ignored_dirs.is_empty()
            && let Ok(tracked) = repo.tracked_directories(&ignored_dirs)
        {
            self.directories.extend(
                ignored_dirs
                    .into_iter()
                    .filter(|dir| !tracked.contains(*dir))
                    .filter(|dir| {
                        // An ignored ancestor covers everything below it.
                        !dir.ancestors()
                            .skip(1)
                            .any(|parent| ignored.contains(parent) && !tracked.contains(parent))
                    })
                    .map(Path::to_owned),
            );
        }
        !paths.iter().all(|path| ignored.contains(path))
    }
}

fn is_directory(repo: &Repo, path: &Path) -> bool {
    fs::symlink_metadata(repo.root().join(path)).is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{tests::fixture_command, watch::Batch};

    #[test]
    #[allow(
        clippy::unwrap_used,
        reason = "Create a temporary repository and exercise its ignore rules"
    )]
    fn caches_only_untracked_directories_and_invalidates_on_rule_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            assert!(
                fixture_command("git")
                    .current_dir(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q"]);
        git(&["config", "core.excludesFile", "/dev/null"]);
        fs::write(root.join(".gitignore"), "/target/\n/mixed/\n").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::create_dir(root.join("mixed")).unwrap();
        fs::write(root.join("mixed/tracked"), "").unwrap();
        git(&["add", "-f", "mixed/tracked"]);
        let repo = Repo::open(&root).unwrap();
        let mut ignore = Ignore::default();
        let event = |ignore: &mut Ignore, path| {
            let mut batch = Batch::default();
            batch.collect_path(&repo, &root.join(path));
            batch.affects_review(&repo, ignore)
        };

        assert!(!event(&mut ignore, "target/build.o"));
        assert!(ignore.directories.contains(Path::new("target")));
        assert!(!event(&mut ignore, "target/nested/file"));
        assert!(event(&mut ignore, "target-other/file"));
        assert!(!event(&mut ignore, "mixed/scratch"));
        assert!(!ignore.directories.contains(Path::new("mixed")));
        assert!(event(&mut ignore, "mixed/tracked"));

        fs::write(root.join(".gitignore"), "").unwrap();
        // The cached prefix still answers without Git until the rule event arrives.
        assert!(!event(&mut ignore, "target/build.o"));
        assert!(event(&mut ignore, ".gitignore"));
        assert!(event(&mut ignore, "target/build.o"));

        let exclude = repo.common_dir().join("info/exclude");
        fs::write(&exclude, "/target/\n").unwrap();
        assert!(event(&mut ignore, ".git/info/exclude"));
        assert!(!event(&mut ignore, "target/build.o"));
        fs::write(&exclude, "").unwrap();
        assert!(event(&mut ignore, ".git/info/exclude"));
        assert!(event(&mut ignore, "target/build.o"));

        let global = tempfile::NamedTempFile::new().unwrap();
        fs::write(global.path(), "/target/\n").unwrap();
        git(&[
            "config",
            "core.excludesFile",
            global.path().to_str().unwrap(),
        ]);
        assert!(event(&mut ignore, ".git/config"));
        assert!(!event(&mut ignore, "target/build.o"));
        fs::write(global.path(), "").unwrap();
        assert!(event(&mut ignore, "target/build.o"));

        fs::write(global.path(), "/target/\n").unwrap();
        assert!(event(&mut ignore, "target/build.o"));
        assert!(!event(&mut ignore, "target/build.o"));
        fs::write(root.join("target/tracked"), "").unwrap();
        git(&["add", "-f", "target/tracked"]);
        assert!(event(&mut ignore, ".git/index"));
        assert!(event(&mut ignore, "target/tracked"));
    }
}
