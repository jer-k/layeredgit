#![allow(
    clippy::unwrap_used,
    reason = "Disposable preferences must be writable"
)]

use std::ffi::OsString;

use super::*;
use crate::{panels::Widths, preferences::Preferences};

#[test]
fn imports_workspace_keys_without_changing_the_source_or_importing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let new_path = dir.path().join("new.json");
    let mut old = Prefs::at(&old_path);
    let settings = Preferences {
        repositories: vec![("/repo".into(), Some("refs/heads/base".into()))],
        selected: Some("/repo".into()),
        collapsed: true,
    };
    settings.save(&mut old).unwrap();
    old.set(WIDTHS_KEY, &[248.0_f32, 300.0]).unwrap();
    old.set("unrelated", &42).unwrap();
    let original = fs::read(&old_path).unwrap();

    let mut new = Prefs::at(&new_path);
    migrate(&mut new, &old_path, None).unwrap();
    let mut new = Prefs::at(&new_path);
    assert_eq!(Preferences::load(&new), settings);
    assert_eq!(Widths::load(&new), Widths::load(&old));
    assert_eq!(new.get::<bool>(MARKER), Some(true));
    assert!(!new.contains("unrelated"));
    assert_eq!(fs::read(&old_path).unwrap(), original);

    new.remove(KEY).unwrap();
    new.remove(WIDTHS_KEY).unwrap();
    migrate(&mut Prefs::at(&new_path), &old_path, None).unwrap();
    assert_eq!(
        Preferences::load(&Prefs::at(&new_path)),
        Preferences::default()
    );
    assert_eq!(Widths::load(&Prefs::at(&new_path)), Widths::default());
}

#[test]
fn existing_workspace_settings_skip_the_import() {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let mut old = Prefs::at(&old_path);
    old.set(RECENTS_KEY, &["/old"].map(OsString::from)).unwrap();
    let mut new = Prefs::at(dir.path().join("new.json"));
    new.set(WIDTHS_KEY, &[270.0_f32, 310.0]).unwrap();

    migrate(&mut new, &old_path, None).unwrap();
    assert!(!new.contains(RECENTS_KEY));
    assert_eq!(new.get::<[f32; 2]>(WIDTHS_KEY), Some([270.0, 310.0]));
    assert_eq!(new.get::<bool>(MARKER), Some(true));
}

#[test]
fn resumes_an_interrupted_import_and_keeps_the_recents_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let mut old = Prefs::at(&old_path);
    old.set(RECENTS_KEY, &["/old"].map(OsString::from)).unwrap();
    old.set(WIDTHS_KEY, &[248.0_f32, 300.0]).unwrap();
    let new_path = dir.path().join("new.json");
    let mut new = Prefs::at(&new_path);
    new.set(MARKER, &false).unwrap();
    new.set(WIDTHS_KEY, &[260.0_f32, 350.0]).unwrap();

    migrate(&mut Prefs::at(&new_path), &old_path, None).unwrap();
    let new = Prefs::at(&new_path);
    assert_eq!(
        Preferences::load(&new).repositories,
        [("/old".into(), None)]
    );
    assert_eq!(new.get::<[f32; 2]>(WIDTHS_KEY), Some([260.0, 350.0]));
    assert_eq!(new.get::<bool>(MARKER), Some(true));
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;

    struct Domain(String);

    impl Drop for Domain {
        fn drop(&mut self) {
            let _ = defaults().args(["delete", &self.0]).output();
        }
    }

    #[test]
    fn imports_appkit_frame_only_into_a_fresh_install() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = format!("local.layeredgit.migration-test.{}", dir.path().display());
        let prefix = prefix.replace('/', ".");
        let old = Domain(format!("{prefix}.old"));
        let new = Domain(format!("{prefix}.new"));
        let populated = Domain(format!("{prefix}.populated"));
        let old_frame = "100 120 1230 750 0 0 1440 900 ";
        let other_frame = "50 80 1100 700 0 0 1440 900 ";
        let old_path = dir.path().join("old.json");
        Prefs::at(&old_path)
            .set(RECENTS_KEY, &["/old"].map(OsString::from))
            .unwrap();
        write_frame(&old.0, old_frame).unwrap();
        write_frame(&populated.0, other_frame).unwrap();

        let mut fresh = Prefs::at(dir.path().join("new.json"));
        migrate(&mut fresh, &old_path, Some((&old.0, &new.0))).unwrap();
        assert_eq!(read_frame(&new.0).unwrap().as_deref(), Some(old_frame));
        assert_eq!(read_frame(&old.0).unwrap().as_deref(), Some(old_frame));
        assert_eq!(fresh.get::<bool>(MARKER), Some(true));
        assert!(fresh.contains(RECENTS_KEY));
        write_frame(&old.0, other_frame).unwrap();
        migrate(&mut fresh, &old_path, Some((&old.0, &new.0))).unwrap();
        assert_eq!(read_frame(&new.0).unwrap().as_deref(), Some(old_frame));

        write_frame(&old.0, old_frame).unwrap();
        let mut existing = Prefs::at(dir.path().join("populated.json"));
        migrate(&mut existing, &old_path, Some((&old.0, &populated.0))).unwrap();
        assert!(!existing.contains(RECENTS_KEY));
        assert_eq!(
            read_frame(&populated.0).unwrap().as_deref(),
            Some(other_frame)
        );
    }
}
