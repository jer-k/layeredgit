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
    let FileBody::Text(diff) = &file.body else {
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
fn filtering_preserves_context_and_three_lines_around_changes() {
    let review = Review::new(demo::review());
    let FileBody::Text(diff) = &review.files[0].body else {
        panic!("expected text")
    };
    assert_eq!(diff.hunks(Filter::All).len(), 2);
    assert_eq!(diff.hunks(Filter::Committed).len(), 2);
    assert_eq!(diff.hunks(Filter::Uncommitted).len(), 1);
    let hunk = diff.hunks(Filter::Uncommitted).first().unwrap();
    let lines = &diff.lines[hunk.clone()];
    let first = lines
        .iter()
        .position(|line| Filter::Uncommitted.appearance(line.change) != Context);
    let last = lines
        .iter()
        .rposition(|line| Filter::Uncommitted.appearance(line.change) != Context);
    assert_eq!(first, Some(3));
    assert_eq!(last, Some(lines.len() - 4));
    assert_eq!(Filter::Uncommitted.appearance(CommittedAdd), Context);
    assert_eq!(Filter::Committed.appearance(UncommittedDelete), Context);
    assert_eq!(
        Filter::Uncommitted.appearance(CommittedThenRemoved),
        CommittedThenRemoved
    );
    assert_eq!(
        Filter::Committed.appearance(CommittedThenRemoved),
        CommittedThenRemoved
    );

    let rows = rows(&review, Filter::Uncommitted);
    assert!(
        rows.rows.iter().any(|row| row.file == 1
            && matches!(row.kind, RowKind::Placeholder("No uncommitted changes")))
    );
}

#[test]
fn folds_expand_only_their_range_and_keep_the_other_keys() {
    let review = Review::new(demo::review());
    let original = rows(&review, Filter::All);
    let fold = original
        .rows
        .iter()
        .find(|row| row.file == 8 && matches!(row.kind, RowKind::Fold(_)))
        .unwrap();
    let RowKind::Fold(range) = &fold.kind else {
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
        files[3].path_label(),
        "docs/review-notes.md → docs/review-guide.md"
    );
    assert_eq!(files[5].path(), "src/legacy.rs");
    for (index, class) in [(4, CommittedAdd), (5, CommittedDelete)] {
        let FileBody::Text(diff) = &files[index].body else {
            panic!("expected text")
        };
        assert!(!diff.lines.is_empty());
        assert!(diff.lines.iter().all(|line| line.change == class));
        assert!(diff.newlines.is_empty());
    }
    let rows = rows(&review, Filter::All);
    assert!(rows.rows.iter().any(
        |row| row.file == 6 && matches!(row.kind, RowKind::Placeholder("Binary file changed"))
    ));
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
    let FileBody::Text(diff) = &review.files[0].body else {
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
        binary: false,
        committed: ChangeKind::Modified,
        uncommitted: ChangeKind::Modified,
    }
}
