#![allow(
    clippy::unwrap_used,
    reason = "Expected review rows must exist in these fixtures"
)]

use std::{
    fmt::Write as _,
    fs,
    sync::{Arc, mpsc},
    time::Duration,
};

use crate::{
    input::{ChangeKind, FileVersion, InputFile, ReviewInput},
    model::FileBody,
};

use super::*;

#[test]
fn refresh_keeps_filter_selection_expanded_fold_and_top_line_after_rename() {
    let before = review(None);
    let mut state = State::new(before, ready(), None);
    let mut tx = Tx::default();
    state.set_filter(Filter::Committed, &mut tx);
    state.toggle_file(0, &mut tx);

    let FileBody::Text(diff) = state.review.files[1].body(Filter::All) else {
        panic!("the selected file should have a text diff");
    };
    let anchor = diff
        .lines
        .iter()
        .position(|line| line.text == "line 21")
        .unwrap();
    let fold = state
        .rows
        .rows
        .iter()
        .find_map(|row| match &row.kind {
            RowKind::Fold { range, .. } if row.file == 1 && range.contains(&anchor) => {
                Some(range.clone())
            }
            _ => None,
        })
        .unwrap();
    state.expand(1, fold, &mut tx);
    let top = state
        .rows
        .rows
        .iter()
        .position(|row| {
            row.file == 1 && matches!(row.kind, RowKind::Line(index) if index == anchor)
        })
        .unwrap();
    state.show_rows(top..state.rows.rows.len());

    state.replace_review(
        Replacement::new(review(Some("a-renamed.txt")), &state.review),
        &mut Tx::default(),
    );
    assert_eq!(state.filter, Filter::Committed);
    assert_eq!(state.active_file, Some(0));
    assert_eq!(state.collapsed, HashSet::from([1]));

    let row = &state.rows.rows[state.visible_rows.start];
    assert_eq!(row.file, 0);
    let RowKind::Line(index) = row.kind else {
        panic!("the line in the expanded fold should still be visible");
    };
    let FileBody::Text(diff) = state.review.files[0].body(Filter::All) else {
        panic!("the renamed file should have a text diff");
    };
    assert_eq!(diff.lines[index].text, "line 21");
}

#[test]
fn filesystem_refresh_reloads_dirty_fixture_and_restores_state() {
    let (_directory, repository) = crate::source::tests::fixture();
    let mut state = State::open(Some(repository.clone()), false, false);
    let (send, received) = mpsc::channel();
    state.setup(Arc::new(move || {
        let _ = send.send(());
    }));
    let mut tx = Tx::default();
    state.request_refresh(&mut tx);
    assert!(!state.poll(0.0, &mut tx));
    let wait = || received.recv_timeout(Duration::from_secs(10)).unwrap();
    wait();
    assert!(state.poll(0.0, &mut tx));
    let find = |state: &State, path| {
        state
            .review
            .files
            .iter()
            .position(|file| file.path() == path)
            .unwrap()
    };
    let generated = find(&state, "src/generated.rs");
    let lib = find(&state, "src/lib.rs");
    state.set_filter(Filter::Uncommitted, &mut tx);
    state.toggle_file(lib, &mut tx);
    let fold = state
        .rows
        .rows
        .iter()
        .find_map(|row| match &row.kind {
            RowKind::Fold { range, .. } if row.file == generated => Some(range.clone()),
            _ => None,
        })
        .unwrap();
    state.expand(generated, fold, &mut tx);
    let top = state.rows.header(generated).unwrap() + 2;
    let key = state.rows.rows[top].key.clone();
    assert!(matches!(state.rows.rows[top].kind, RowKind::Line(_)));
    state.show_rows(top..top + 10);

    let lib_path = repository.path.join("src/lib.rs");
    let lib_text = fs::read_to_string(&lib_path).unwrap() + "\n// Tracked file edited again\n";
    fs::write(lib_path, lib_text).unwrap();
    fs::write(
        repository.path.join("TODO.md"),
        "untracked file edited again\n",
    )
    .unwrap();

    // The virtual clock stays short of the fallback poll. Only a real watcher
    // notification can make this reload and wake us when it is ready.
    let contents = |state: &State, path, expected| {
        let file = &state.review.files[find(state, path)];
        matches!(file.body(Filter::Uncommitted), FileBody::Text(diff)
            if diff.lines.iter().any(|line| line.text == expected))
    };
    loop {
        wait();
        state.poll(0.25, &mut tx);
        if contents(&state, "TODO.md", "untracked file edited again")
            && contents(&state, "src/lib.rs", "// Tracked file edited again")
        {
            break;
        }
    }
    assert_eq!(state.filter, Filter::Uncommitted);
    assert_eq!(state.active_file, Some(generated));
    assert!(state.collapsed.contains(&lib));
    assert_eq!(state.rows.rows[state.visible_rows.start].key, key);
    assert_eq!(state.expanded.len(), 1);
}

fn review(renamed: Option<&str>) -> Review {
    let mut base = String::new();
    let mut head = String::new();
    for number in 1..=50 {
        let _ = writeln!(base, "line {number}");
        if number == 5 || number == 40 {
            let _ = writeln!(head, "committed line {number}");
        } else {
            let _ = writeln!(head, "line {number}");
        }
    }
    let context = file("other.txt", "other.txt", ["old\n", "new\n", "new\n"]);
    let files = if let Some(renamed) = renamed {
        let working = format!("inserted in worktree\n{head}");
        vec![
            file("original.txt", renamed, [&base, &head, &working]),
            context,
        ]
    } else {
        vec![
            context,
            file("original.txt", "original.txt", [&base, &head, &head]),
        ]
    };
    Review::new(ReviewInput {
        base_revision: [None, None],
        base_label: "main".into(),
        head_label: "feature".into(),
        files,
    })
}

fn file(original: &str, latest: &str, texts: [&str; 3]) -> InputFile {
    let [base, head, worktree] = texts;
    let version = |path: &str, text: &str| {
        Some(FileVersion {
            path: path.into(),
            text: Some(text.into()),
        })
    };
    InputFile {
        base: version(original, base),
        head: version(original, head),
        worktree: version(latest, worktree),
        placeholders: [None; 2],
        committed: ChangeKind::Modified,
        uncommitted: if original == latest {
            ChangeKind::Unchanged
        } else {
            ChangeKind::Renamed
        },
    }
}
