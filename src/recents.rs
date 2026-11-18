//! Recently opened working trees, saved by the launch worker using Rune preferences.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use rune::prefs::{Prefs, PrefsError};

const KEY: &str = "recent_repositories";
const LIMIT: usize = 8;

pub(crate) struct Recents {
    prefs: Prefs,
    paths: Vec<PathBuf>,
}

impl Recents {
    pub(crate) fn new(prefs: Prefs) -> Self {
        let mut paths = Vec::new();
        for path in prefs.get::<Vec<OsString>>(KEY).unwrap_or_default() {
            let path = PathBuf::from(path);
            if path.is_absolute() && !paths.contains(&path) {
                paths.push(path);
            }
            if paths.len() == LIMIT {
                break;
            }
        }
        Self { prefs, paths }
    }

    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub(crate) fn visit(&mut self, path: &Path) -> Result<(), PrefsError> {
        self.paths.retain(|previous| previous != path);
        self.paths.insert(0, path.to_owned());
        self.paths.truncate(LIMIT);
        let stored: Vec<_> = self.paths.iter().map(|path| path.as_os_str()).collect();
        self.prefs.set(KEY, &stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, reason = "The temporary preferences are writable")]
    fn deduplicates_caps_and_persists_most_recent_first() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefs.json");
        let mut recents = Recents::new(Prefs::at(&path));
        for index in 0..=LIMIT {
            recents.visit(Path::new(&format!("/repo-{index}"))).unwrap();
        }
        recents.visit(Path::new("/repo-3")).unwrap();
        let reopened = Recents::new(Prefs::at(&path));
        let expected =
            [3, 8, 7, 6, 5, 4, 2, 1].map(|index| PathBuf::from(format!("/repo-{index}")));
        assert_eq!(recents.paths(), expected);
        assert_eq!(reopened.paths(), expected);
    }
}
