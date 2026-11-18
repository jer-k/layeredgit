#![allow(
    clippy::unwrap_used,
    reason = "Expected review rows must exist in these fixtures"
)]

use std::fmt::Write as _;

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

    state.replace_review(review(Some("a-renamed.txt")), &mut Tx::default());
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
    assert_eq!(
        state.previous_review.as_ref().unwrap().files[1].path(),
        "original.txt"
    );
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
