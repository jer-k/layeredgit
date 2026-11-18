use std::collections::HashSet;

use diff_stack::ThreeWayChange::{
    CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd, UncommittedDelete,
};
use rune_kit::syntax::{Paint, Syntax};

use crate::{
    demo,
    input::{ChangeKind, FileVersion, InputFile},
};

use super::*;

#[test]
fn demo_covers_all_lifetimes_and_counts_them_per_transition() {
    let review = Review::new(demo::review());
    let file = &review.files[0];
    let FileBody::Text(diff) = file.body(Filter::All) else {
        panic!("expected text")
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
            diff.lines.iter().any(|line| line.change == class),
            "missing {class:?}"
        );
    }
    let interim = diff
        .lines
        .iter()
        .find(|line| line.change == CommittedThenRemoved)
        .unwrap();
    assert_eq!(
        (interim.base, interim.head, interim.worktree),
        (None, Some(9), None)
    );
    assert_eq!(
        file.committed_counts,
        Counts {
            added: 3,
            removed: 2
        }
    );
    assert_eq!(
        file.uncommitted_counts,
        Counts {
            added: 3,
            removed: 3
        }
    );
}

#[test]
fn pair_projections_classify_lifetimes_and_use_the_pair_line_numbers() {
    let review = Review::new(demo::review());
    let FileBody::Text(diff) = review.files[0].body(Filter::All) else {
        panic!("expected text")
    };
    for (all, committed, working) in [
        (Context, Some(Context), Some(Context)),
        (CommittedAdd, Some(CommittedAdd), Some(Context)),
        (CommittedDelete, Some(CommittedDelete), None),
        (
            CommittedThenRemoved,
            Some(CommittedAdd),
            Some(UncommittedDelete),
        ),
        (UncommittedAdd, None, Some(UncommittedAdd)),
        (UncommittedDelete, Some(Context), Some(UncommittedDelete)),
    ] {
        let line = diff.lines.iter().find(|line| line.change == all).unwrap();
        assert_eq!(Filter::All.change(line), Some(all));
        assert_eq!(Filter::Committed.change(line), committed);
        assert_eq!(Filter::Uncommitted.change(line), working);
    }

    let review = review_of(text_file([
        "one\ntwo\nthree\nbase\nfive\nsix\nseven\neight\nnine\n",
        "one\ntwo\nthree\nhead only\nfive\nsix\nseven\neight\nnine\n",
        "one\ntwo\nthree\nwork a\nwork b\nwork c\nwork d\nwork e\nfive\nsix\nseven\neight\nnine\n",
    ]));
    let FileBody::Text(diff) = review.files[0].body(Filter::All) else {
        panic!("expected text")
    };
    for (filter, header, texts) in [
        (
            Filter::Committed,
            "@@ -1,7 +1,7 @@",
            vec![
                "one",
                "two",
                "three",
                "base",
                "head only",
                "five",
                "six",
                "seven",
            ],
        ),
        (
            Filter::Uncommitted,
            "@@ -1,7 +1,11 @@",
            vec![
                "one",
                "two",
                "three",
                "head only",
                "work a",
                "work b",
                "work c",
                "work d",
                "work e",
                "five",
                "six",
                "seven",
            ],
        ),
    ] {
        let rows = rows(&review, filter);
        assert_eq!(rows.hunks.len(), 1);
        assert!(
            matches!(&rows.rows[rows.hunks[0]].kind, RowKind::Hunk { label, .. } if label == header)
        );
        assert_eq!(
            rows.rows
                .iter()
                .filter_map(|row| match row.kind {
                    RowKind::Line(index) => Some(diff.lines[index].text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            texts
        );
        assert!(
            rows.rows
                .iter()
                .any(|row| matches!(row.kind, RowKind::Fold { count: 2, .. }))
        );
    }

    let demo = Review::new(demo::review());
    assert!(!rows(&demo, Filter::Uncommitted).files.contains(&1));
}

#[test]
fn unreadable_worktree_still_allows_the_committed_projection() {
    let mut file = text_file(["old\n", "new\n", ""]);
    file.placeholders = [None, Some(Placeholder::Binary)];
    file.worktree.as_mut().unwrap().text = None;
    let review = review_of(file);
    let file = &review.files[0];
    assert!(matches!(
        file.body(Filter::All),
        FileBody::Placeholder(Placeholder::Binary)
    ));
    assert!(matches!(
        file.body(Filter::Uncommitted),
        FileBody::Placeholder(Placeholder::Binary)
    ));
    assert_eq!(
        file.committed_counts,
        Counts {
            added: 1,
            removed: 1
        }
    );
    assert_eq!(rows(&review, Filter::Committed).hunks.len(), 1);
}

#[test]
fn folds_expand_only_their_range_and_keep_the_other_keys() {
    let review = Review::new(demo::review());
    let original = rows(&review, Filter::All);
    let fold = original
        .rows
        .iter()
        .find(|row| row.file == 8 && matches!(row.kind, RowKind::Fold { .. }))
        .unwrap();
    let RowKind::Fold { range, .. } = &fold.kind else {
        panic!("expected a fold")
    };
    assert!(range.len() > 100);
    let expanded = ReviewRows::new(
        &review,
        Filter::All,
        &HashSet::new(),
        &HashSet::from([(fold.file, range.start, range.end)]),
    );
    assert_eq!(expanded.rows.len(), original.rows.len() + range.len() - 1);
    assert_eq!(expanded.hunks.len(), original.hunks.len());
    assert!(original.rows.iter().filter(|row| row.key != fold.key).map(|row| &row.key).eq(
        expanded.rows.iter().filter(|row| !matches!(row.kind, RowKind::Line(n) if row.file == fold.file && range.contains(&n))).map(|row| &row.key)
    ));
    let collapsed = ReviewRows::new(&review, Filter::All, &HashSet::from([8]), &HashSet::new());
    assert_eq!(collapsed.rows.iter().filter(|row| row.file == 8).count(), 2);
    assert!(collapsed.hunks.len() < original.hunks.len());
}

#[test]
fn added_deleted_renamed_and_binary_files_keep_their_review_metadata() {
    let review = Review::new(demo::review());
    let files = &review.files;
    assert_eq!(
        files[3].path_label(Filter::All),
        "docs/review-notes.md → docs/review-guide.md"
    );
    assert_eq!(files[5].path(), "src/legacy.rs");
    for (index, class) in [(4, CommittedAdd), (5, CommittedDelete)] {
        let FileBody::Text(diff) = files[index].body(Filter::All) else {
            panic!("expected text")
        };
        assert!(!diff.lines.is_empty());
        assert!(diff.lines.iter().all(|line| line.change == class));
        assert!(diff.newlines.is_empty());
    }
    let rows = rows(&review, Filter::All);
    assert!(rows.rows.iter().any(|row| row.file == 6
        && matches!(&row.kind, RowKind::Placeholder(message) if message == "Binary file")));
}

#[test]
fn newline_only_diff_marks_the_unterminated_side_in_the_right_filter() {
    let review = Review::new(demo::review());
    let has_marker = |filter| {
        rows(&review, filter)
            .rows
            .into_iter()
            .any(|row| row.file == 7 && matches!(row.kind, RowKind::Newline("Working tree")))
    };
    assert!(has_marker(Filter::All));
    assert!(has_marker(Filter::Uncommitted));
    assert!(!has_marker(Filter::Committed));

    let file = text_file(["one\ntwo", "one\ntwo\n", "one"]);
    let review = review_of(file);
    let all = rows(&review, Filter::All);
    let markers: Vec<_> = all
        .rows
        .iter()
        .filter_map(|row| match row.kind {
            RowKind::Newline(version) => Some(version),
            _ => None,
        })
        .collect();
    assert_eq!(markers, ["Working tree", "Base"]);
}

#[test]
fn same_unterminated_head_gets_one_marker_for_both_transitions() {
    let review = review_of(text_file(["text\n", "text", "text\n"]));
    for filter in Filter::ALL {
        let rows = rows(&review, filter);
        let markers = rows
            .rows
            .iter()
            .filter(|row| matches!(row.kind, RowKind::Newline("HEAD")));
        assert_eq!(markers.count(), 1);
    }
}

#[test]
fn newline_marker_stays_visible_across_insertions_in_the_other_layer() {
    let review = review_of(text_file([
        "keep",
        "keep\nlast\n",
        "keep\none\ntwo\nthree\nfour\nfive\nlast\n",
    ]));
    let committed = rows(&review, Filter::Committed);
    let marker = committed
        .rows
        .iter()
        .position(|row| matches!(row.kind, RowKind::Newline("Base")));
    assert!(matches!(
        marker
            .and_then(|index| committed.rows.get(index - 1))
            .map(|row| &row.kind),
        Some(RowKind::Line(0))
    ));
}

#[test]
fn syntax_uses_last_living_version_and_whole_version_context() {
    let review = review_of(text_file([
        "/*\r\n\tlet base = \"é\";\r\n*/\r\nanchor\r\nlet shared = 9;\r\n",
        "anchor\r\n/*\r\n\tlet interim = 7;\r\nlet shared = 9;\r\n*/\r\n",
        "anchor\r\nlet shared = 9;\r\n",
    ]));
    let FileBody::Text(diff) = review.files[0].body(Filter::All) else {
        panic!("expected text")
    };
    let find = |word| {
        diff.lines
            .iter()
            .find(|line| line.text.contains(word))
            .unwrap()
    };
    for (word, expected) in [
        ("let base", CommittedDelete),
        ("let interim", CommittedThenRemoved),
    ] {
        let line = find(word);
        assert_eq!(line.change, expected);
        assert!(line.text.starts_with("    "));
        assert!(line.highlights.iter().any(|(range, paint)| {
            *paint == Paint::Kind(Syntax::Comment) && line.text[range.clone()].contains(word)
        }));
        assert!(
            line.highlights
                .iter()
                .all(|(range, _)| line.text.get(range.clone()).is_some())
        );
    }
    let shared = find("let shared");
    assert_eq!(shared.change, Context);
    assert!(
        shared
            .highlights(Filter::Committed)
            .iter()
            .any(|(_, paint)| { *paint == Paint::Kind(Syntax::Comment) })
    );
    assert!(
        shared.highlights.iter().any(|(range, paint)| {
            *paint == Paint::Kind(Syntax::Keyword) && range.start == 0 && range.end >= "let".len()
        }),
        "{:?}",
        shared.highlights
    );
}

fn rows(review: &Review, filter: Filter) -> ReviewRows {
    ReviewRows::new(review, filter, &HashSet::new(), &HashSet::new())
}

fn review_of(file: InputFile) -> Review {
    Review::new(ReviewInput {
        base_revision: [None, None],
        base_label: "base".into(),
        head_label: "head".into(),
        files: vec![file],
    })
}

fn text_file(texts: [&str; 3]) -> InputFile {
    let [base, head, worktree] = texts.map(|text| {
        Some(FileVersion {
            path: "example.rs".into(),
            text: Some(text.into()),
        })
    });
    InputFile {
        base,
        head,
        worktree,
        placeholders: [None; 2],
        committed: ChangeKind::Modified,
        uncommitted: ChangeKind::Modified,
    }
}

#[test]
fn editing_a_clean_file_pulses_only_the_edited_line() {
    let head = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let edited = head.replace("line 7\n", "line seven\n");
    let clean = |worktree: &str, uncommitted| InputFile {
        base: Some(FileVersion {
            path: "a.txt".into(),
            text: Some(head.clone()),
        }),
        head: Some(FileVersion {
            path: "a.txt".into(),
            text: Some(head.clone()),
        }),
        worktree: Some(FileVersion {
            path: "a.txt".into(),
            text: Some(worktree.to_owned()),
        }),
        placeholders: [None; 2],
        committed: ChangeKind::Unchanged,
        uncommitted,
    };
    let mut before = demo::review();
    let mut after = before.clone();
    // The file is clean before the reload, so the previous review omits it.
    before.files.clear();
    after.files = vec![clean(&edited, ChangeKind::Modified)];

    let replacement = fresh::Replacement::new(Review::new(after), &Review::new(before));
    assert_eq!(replacement.fresh.get(&0), Some(&HashSet::from([7])));

    // A file that is new in the working tree still pulses entirely.
    let mut created = demo::review();
    created.files = vec![InputFile {
        base: None,
        head: None,
        committed: ChangeKind::Unchanged,
        uncommitted: ChangeKind::Added,
        ..clean("x\ny\n", ChangeKind::Added)
    }];
    let mut empty = demo::review();
    empty.files.clear();
    let replacement = fresh::Replacement::new(Review::new(created), &Review::new(empty));
    assert_eq!(replacement.fresh.get(&0), Some(&HashSet::from([1, 2])));
}
