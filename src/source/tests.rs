#![allow(
    clippy::unwrap_used,
    reason = "Fixture setup and expectations may fail the test"
)]

use std::{
    collections::HashSet,
    convert::Infallible,
    fs,
    process::Command,
    sync::{Arc, mpsc},
    time::Duration,
};

use diff_stack::ThreeWayChange::{
    CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd, UncommittedDelete,
};
use repo::{Change, FileKind};
use rune::Tx;

use crate::model::{FileBody, Filter, Review, ReviewRows, RowKind};

use super::*;

#[test]
fn maps_version_contents_and_preserves_rename_paths() {
    let change = |kind, binary| Change {
        kind,
        binary,
        mode_changed: false,
        lines: None,
    };
    let version = |path: &str| repo::FileVersion {
        path: path.into(),
        kind: FileKind::Regular,
    };
    let mut file = ChangedFile {
        base: Some(version("old")),
        head: Some(version("new")),
        worktree: Some(version("new")),
        committed: change(repo::ChangeKind::Renamed, false),
        uncommitted: change(repo::ChangeKind::TypeChanged, false),
    };
    let convert = |file: &ChangedFile, worktree: Contents| {
        convert_file(file, |layer| {
            Ok::<_, Infallible>(match layer {
                Layer::Base | Layer::Head => Contents::Bytes(b"hello\n".to_vec()),
                Layer::Worktree => worktree.clone(),
            })
        })
        .unwrap()
    };
    let normal = convert(&file, Contents::Missing);
    assert_eq!(normal.base.unwrap().path, Path::new("old"));
    assert_eq!(normal.head.unwrap().text.as_deref(), Some("hello\n"));
    assert!(normal.worktree.is_none());
    assert_eq!(normal.placeholders, [None; 2]);
    assert_eq!(normal.committed, ChangeKind::Renamed);
    assert_eq!(normal.uncommitted, ChangeKind::ModeChanged);
    for (contents, expected, message) in [
        (
            Contents::Bytes(b"bin\0ary".to_vec()),
            Placeholder::Binary,
            "Binary file",
        ),
        (
            Contents::Bytes(vec![0xff]),
            Placeholder::NotUtf8,
            "Not UTF-8",
        ),
        (Contents::Submodule, Placeholder::Submodule, "Submodule"),
        (
            Contents::TooLarge {
                size: 12_300_000,
                limit: repo::DEFAULT_BYTE_LIMIT,
            },
            Placeholder::TooLarge(12_300_000),
            "File too large (12.3 MB)",
        ),
    ] {
        let input = convert(&file, contents);
        let version = input.worktree.unwrap();
        assert_eq!(version.path, Path::new("new"));
        assert!(version.text.is_none());
        assert_eq!(input.placeholders, [None, Some(expected)]);
        assert_eq!(expected.message(), message);
    }
    file.uncommitted.binary = true;
    assert_eq!(
        convert(
            &file,
            Contents::Bytes(b"text forced binary by Git".to_vec())
        )
        .placeholders,
        [None, Some(Placeholder::Binary)],
    );
}

#[test]
fn fixture_repository_builds_a_three_way_review() {
    let (_directory, mut target) = fixture();
    let input = target.load_input().unwrap();
    assert_eq!(
        (&*input.base_label, &*input.head_label),
        ("main", "feature")
    );
    let review = Review::new(input);
    target.base = Some("refs/heads/main".into());
    assert_eq!(target.load_input().unwrap().base_label, "refs/heads/main");

    assert!(
        !review
            .files
            .iter()
            .any(|file| file.path() == "docs/guide.md")
    );
    let find = |path| {
        review
            .files
            .iter()
            .position(|file| file.path() == path)
            .unwrap()
    };
    let FileBody::Text(lib) = review.files[find("src/lib.rs")].body(Filter::All) else {
        panic!("src/lib.rs should have a text diff");
    };
    for class in [
        Context,
        CommittedAdd,
        CommittedDelete,
        UncommittedAdd,
        UncommittedDelete,
        CommittedThenRemoved,
    ] {
        assert!(
            lib.lines.iter().any(|line| line.change == class),
            "missing {class:?}"
        );
    }
    let rename = &review.files[find("docs/CHANGELOG.md")];
    assert_eq!(
        rename.path_label(Filter::All),
        "docs/old-name.md → docs/CHANGELOG.md"
    );
    assert_eq!(rename.committed, ChangeKind::Renamed);
    assert_eq!(
        review
            .files
            .iter()
            .filter(|file| file
                .paths()
                .any(|path| path == Path::new("docs/old-name.md")))
            .count(),
        1
    );
    assert!(matches!(
        review.files[find("assets/logo.png")].body(Filter::All),
        FileBody::Placeholder(Placeholder::Binary)
    ));

    let notes = find("notes.txt");
    let rows = ReviewRows::new(&review, Filter::All, &HashSet::new(), &HashSet::new());
    assert!(
        rows.rows
            .iter()
            .any(|row| row.file == notes && matches!(row.kind, RowKind::Newline("Working tree")))
    );
    assert_eq!(review.files[notes].uncommitted_counts.added, 0);
    assert_eq!(review.files[notes].uncommitted_counts.removed, 0);
    assert_eq!(
        review.files[notes]
            .summary(Filter::Uncommitted)
            .and_then(|(_, label)| label)
            .as_deref(),
        Some("Newline at end of file")
    );
    for (path, label) in [
        ("web/reference.txt", "Renamed"),
        ("web/setup.sh", "Mode change"),
    ] {
        assert_eq!(
            review.files[find(path)]
                .summary(Filter::Committed)
                .and_then(|(_, label)| label)
                .as_deref(),
            Some(label)
        );
    }

    for path in ["TODO.md", "src/metrics.rs"] {
        let file = &review.files[find(path)];
        assert_eq!(file.committed, ChangeKind::Unchanged, "{path}");
        assert_eq!(file.uncommitted, ChangeKind::Added, "{path}");
        let FileBody::Text(diff) = file.body(Filter::All) else {
            panic!("{path} should have a text diff");
        };
        assert!(!diff.lines.is_empty(), "{path}");
        assert!(
            diff.lines.iter().all(|line| line.change == UncommittedAdd),
            "{path}"
        );
    }
}

#[test]
fn refresh_checks_the_token_and_can_force_a_reload() {
    let (_directory, target) = fixture();
    let mut refresh = Refresh::new(target.clone());
    let (send, received) = mpsc::channel();
    refresh.setup(Arc::new(move || {
        let _ = send.send(());
    }));
    let wait = || received.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut tx = Tx::default();

    refresh.force(&mut tx);
    assert!(refresh.poll(0.0, &mut tx).is_none());
    wait();
    let first = refresh.poll(0.0, &mut tx).unwrap().unwrap();
    assert_eq!(first.files.len(), 13);

    fs::write(target.path.join("TODO.md"), "updated scratch notes\n").unwrap();
    assert!(refresh.poll(1.0, &mut tx).is_none());
    wait();
    assert!(refresh.poll(1.0, &mut tx).is_none());

    refresh.force(&mut tx);
    assert!(refresh.poll(1.1, &mut tx).is_none());
    wait();
    let forced = refresh.poll(1.1, &mut tx).unwrap().unwrap();
    let todo = forced
        .files
        .iter()
        .find(|file| file.path() == "TODO.md")
        .unwrap();
    let FileBody::Text(diff) = todo.body(Filter::All) else {
        panic!("the refreshed untracked file should have a text diff");
    };
    assert_eq!(diff.lines[0].text, "updated scratch notes");

    fs::write(target.path.join("new-untracked.txt"), "added\n").unwrap();
    assert!(refresh.poll(2.2, &mut tx).is_none());
    wait();
    let automatic = refresh.poll(2.2, &mut tx).unwrap().unwrap();
    assert!(
        automatic
            .files
            .iter()
            .any(|file| file.path() == "new-untracked.txt")
    );
}

fn fixture() -> (tempfile::TempDir, Repository) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture repo");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/make-fixture-repo.sh");
    let output = Command::new("sh")
        .arg(script)
        .arg(&path)
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (directory, Repository { path, base: None })
}
