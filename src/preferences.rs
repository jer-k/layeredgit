//! Persistent workspace settings, read and written by the repository worker.

pub(crate) mod migration;

use std::{ffi::OsString, path::PathBuf};

use rune::prefs::{Prefs, PrefsError};

const KEY: &str = "repository_sidebar";
const RECENTS_KEY: &str = "recent_repositories";
type StoredRepository = (PathBuf, Option<OsString>);
type Stored = (Vec<(OsString, Option<OsString>)>, Option<OsString>, bool);

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct Preferences {
    pub(crate) repositories: Vec<StoredRepository>,
    pub(crate) selected: Option<PathBuf>,
    pub(crate) collapsed: bool,
}

impl Preferences {
    pub(crate) fn load(prefs: &Prefs) -> Self {
        let mut paths = Vec::new();
        let (repositories, selected, collapsed) = prefs.get::<Stored>(KEY).unwrap_or_else(|| {
            let previous = prefs.get::<Vec<OsString>>(RECENTS_KEY).unwrap_or_default();
            (
                previous.into_iter().map(|path| (path, None)).collect(),
                None,
                false,
            )
        });
        for (path, base) in repositories {
            let path = PathBuf::from(path);
            if path.is_absolute() && !paths.iter().any(|(previous, _)| previous == &path) {
                paths.push((path, base));
            }
        }
        Self {
            repositories: paths,
            selected: selected.map(PathBuf::from),
            collapsed,
        }
    }

    pub(crate) fn save(&self, prefs: &mut Prefs) -> Result<(), PrefsError> {
        let paths: Vec<_> = self
            .repositories
            .iter()
            .map(|(path, base)| (path.as_os_str(), base))
            .collect();
        let selected = self.selected.as_deref().map(std::path::Path::as_os_str);
        prefs.set(KEY, &(paths, selected, self.collapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, reason = "Temporary preferences must be writable")]
    fn migrates_recents_and_never_restores_deleted_entries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefs.json");
        let mut prefs = Prefs::at(&path);
        prefs
            .set(
                RECENTS_KEY,
                &["/one", "/two", "/one", "relative"].map(OsString::from),
            )
            .unwrap();
        let mut settings = Preferences::load(&prefs);
        assert_eq!(
            settings.repositories,
            [("/one".into(), None), ("/two".into(), None)]
        );
        settings.repositories.remove(0);
        if let Some((_, base)) = settings.repositories.first_mut() {
            *base = Some("main".into());
        }
        settings.selected = Some("/two".into());
        settings.collapsed = true;
        settings.save(&mut prefs).unwrap();
        assert_eq!(Preferences::load(&Prefs::at(&path)), settings);
        settings.repositories.clear();
        settings.save(&mut prefs).unwrap();
        assert_eq!(Preferences::load(&Prefs::at(&path)).repositories, []);
    }
}
