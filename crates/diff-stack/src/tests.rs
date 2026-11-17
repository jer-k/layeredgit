use proptest::prelude::*;

use super::{Stack, ThreeWayChange, align};

fn texts<'a>(stack: &Stack<'a>) -> Vec<&'a str> {
    stack.lines().iter().map(super::DisplayLine::text).collect()
}

#[test]
fn classifies_all_three_version_lifetimes() {
    use ThreeWayChange::{
        CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd,
        UncommittedDelete,
    };

    let stack = align(&[
        "context\ncommitted deletion\nworktree deletion\n",
        "context\ncommitted addition\nshort-lived addition\nworktree deletion\n",
        "context\ncommitted addition\nworktree addition\n",
    ])
    .unwrap();
    let expected = [
        ("context", 0, None, [Some(1), Some(1), Some(1)], Context),
        (
            "committed deletion",
            0,
            Some(1),
            [Some(2), None, None],
            CommittedDelete,
        ),
        (
            "committed addition",
            1,
            None,
            [None, Some(2), Some(2)],
            CommittedAdd,
        ),
        (
            "short-lived addition",
            1,
            Some(2),
            [None, Some(3), None],
            CommittedThenRemoved,
        ),
        (
            "worktree deletion",
            0,
            Some(2),
            [Some(3), Some(4), None],
            UncommittedDelete,
        ),
        (
            "worktree addition",
            2,
            None,
            [None, None, Some(3)],
            UncommittedAdd,
        ),
    ];

    assert_eq!(stack.lines().len(), expected.len());
    for (index, (line, (text, born, dies, numbers, class))) in
        stack.lines().iter().zip(expected).enumerate()
    {
        assert_eq!(line.text(), text);
        assert_eq!((line.born(), line.dies()), (born, dies));
        assert_eq!([0, 1, 2].map(|version| line.line_number(version)), numbers);
        assert_eq!(line.line_number(3), None);
        assert_eq!(stack.classify_three_way(index), Some(class));
    }
    assert_eq!(stack.classify_three_way(expected.len()), None);
    assert_eq!(stack.hunks(0, Some(1..2)), vec![1..4]);
    assert_eq!(stack.hunks(0, Some(2..3)), vec![3..6]);
    assert_eq!(stack.hunks(1, Some(2..3)), vec![2..6]);
    assert_eq!(stack.hunks(0, Some(0..1)), []);
}

#[test]
fn restored_text_gets_a_new_lifetime() {
    let stack = align(&["same\n", "", "same\n"]).unwrap();
    let [deleted, restored] = stack.lines() else {
        panic!("expected one deletion and one insertion");
    };

    assert_eq!(deleted.text(), restored.text());
    assert_eq!((deleted.born(), deleted.dies()), (0, Some(1)));
    assert_eq!((restored.born(), restored.dies()), (2, None));
    assert_eq!(deleted.line_number(2), None);
    assert_eq!(restored.line_number(2), Some(1));
}

#[test]
fn inserts_after_earlier_and_current_deletions() {
    let stack = align(&[
        "old top\nfirst anchor\nold middle\nlast middle\nsecond anchor\nold end",
        "first anchor\nlast middle\nsecond anchor",
        "new top\nfirst anchor\nnew middle one\nnew middle two\nsecond anchor\nnew end",
    ])
    .unwrap();

    assert_eq!(
        texts(&stack),
        [
            "old top",
            "new top",
            "first anchor",
            "old middle",
            "last middle",
            "new middle one",
            "new middle two",
            "second anchor",
            "old end",
            "new end"
        ],
    );
    assert_eq!(stack.lines()[3].dies(), Some(1));
    assert_eq!(stack.lines()[4].dies(), Some(2));
}

#[test]
fn single_version_contains_only_context() {
    let source = String::from("first\n\nlast\n");
    let stack = align(&[&source]).unwrap();
    assert_eq!(stack.version_count(), 1);
    assert_eq!(texts(&stack), ["first", "", "last"]);
    for (line, number) in stack.lines().iter().zip(1..) {
        assert_eq!((line.born(), line.dies()), (0, None));
        assert_eq!(line.line_number(0), Some(number));
        assert_eq!(line.line_number(1), None);
    }
    assert_eq!(stack.lines()[0].text().as_ptr(), source.as_ptr());
    assert_eq!(stack.classify_three_way(0), None);
    assert_eq!(stack.hunks(3, None), []);
}

#[test]
fn two_versions_produce_a_plain_diff() {
    let stack = align(&["old\nkeep\n", "new\nkeep\n"]).unwrap();
    assert_eq!(texts(&stack), ["old", "new", "keep"]);
    assert_eq!(stack.lines()[0].dies(), Some(1));
    assert_eq!(stack.lines()[1].born(), 1);
    assert_eq!(stack.lines()[2].line_number(1), Some(2));
    assert_eq!(stack.classify_three_way(0), None);
    assert_eq!(stack.hunks(0, None), vec![0..2]);
    assert_eq!(stack.hunks(1, Some(1..2)), vec![0..3]);
}

#[test]
fn crlf_changes_comparison_and_is_stripped_for_display() {
    let stack = align(&["word\r\nkeep\n\r\nlast\r", "word\nkeep\n\nlast"]).unwrap();
    assert_eq!(
        texts(&stack),
        ["word", "word", "keep", "", "last", "", "last"]
    );
    let lifetimes: Vec<_> = stack
        .lines()
        .iter()
        .map(|line| (line.born(), line.dies()))
        .collect();
    assert_eq!(
        lifetimes,
        [
            (0, Some(1)),
            (1, None),
            (0, None),
            (0, Some(1)),
            (0, Some(1)),
            (1, None),
            (1, None)
        ]
    );
    assert_eq!(stack.trailing_newline_changed(1), Some(false));
}

#[test]
fn missing_trailing_newline_is_separate_metadata() {
    let stack = align(&["same\n", "same", "same\n", "same\n"]).unwrap();
    assert_eq!(texts(&stack), ["same"]);
    assert_eq!(
        (stack.lines()[0].born(), stack.lines()[0].dies()),
        (0, None)
    );
    assert_eq!(
        [0, 1, 2, 3, 4].map(|version| stack.trailing_newline(version)),
        [Some(true), Some(false), Some(true), Some(true), None],
    );
    assert_eq!(
        [0, 1, 2, 3, 4].map(|version| stack.trailing_newline_changed(version)),
        [None, Some(true), Some(true), Some(false), None],
    );
    assert_eq!(stack.hunks(2, None), vec![0..1]);
    assert_eq!(stack.hunks(0, Some(3..4)), []);

    let emptied = align(&["first\nlast\n", ""]).unwrap();
    assert_eq!(emptied.hunks(0, None), vec![0..2]);
}

#[test]
fn empty_versions_have_zero_lines() {
    let no_versions = align(&[]).unwrap();
    assert_eq!(no_versions.version_count(), 0);
    assert_eq!(no_versions.lines(), []);

    let stack = align(&["", "", ""]).unwrap();
    assert_eq!(stack.version_count(), 3);
    assert_eq!(stack.trailing_newline(0), Some(false));
    assert_eq!(stack.trailing_newline_changed(1), Some(false));
    assert_eq!(stack.lines(), []);
    assert_eq!(stack.hunks(usize::MAX, None), []);

    let blank = align(&["\n"]).unwrap();
    assert_eq!(texts(&blank), [""]);
    assert_eq!(blank.trailing_newline(0), Some(true));
}

#[test]
fn handles_file_creation_and_deletion() {
    let created = align(&["", "first\n", "first\nsecond\n"]).unwrap();
    assert_eq!(texts(&created), ["first", "second"]);
    assert_eq!(
        created.classify_three_way(0),
        Some(ThreeWayChange::CommittedAdd)
    );
    assert_eq!(
        created.classify_three_way(1),
        Some(ThreeWayChange::UncommittedAdd)
    );

    let deleted = align(&["first\n", "first\nsecond\n", ""]).unwrap();
    assert_eq!(texts(&deleted), ["first", "second"]);
    assert_eq!(
        deleted.classify_three_way(0),
        Some(ThreeWayChange::UncommittedDelete)
    );
    assert_eq!(
        deleted.classify_three_way(1),
        Some(ThreeWayChange::CommittedThenRemoved)
    );
    assert_eq!(deleted.lines()[0].line_number(2), None);
    assert_eq!(deleted.lines()[1].line_number(2), None);
}

#[test]
fn context_merges_hunks_when_their_ranges_touch() {
    let stack = align(&[
        "zero\nold one\ntwo\nthree\nold four\nfive\nsix\nseven\neight\nold nine\nten\n",
        "zero\nnew one\ntwo\nthree\nnew four\nfive\nsix\nseven\neight\nnew nine\nten\n",
    ])
    .unwrap();

    assert_eq!(stack.hunks(0, None), vec![1..3, 5..7, 11..13]);
    assert_eq!(stack.hunks(1, None), vec![0..8, 10..14]);
    assert_eq!(stack.hunks(2, None), vec![0..14]);
    assert_eq!(stack.hunks(usize::MAX, None), vec![0..14]);
}

#[test]
fn aligns_many_separate_changes_across_ten_thousand_lines() {
    use std::fmt::Write as _;

    let mut base = String::new();
    let mut head = String::new();
    let mut worktree = String::new();
    for index in 0..5_000 {
        writeln!(base, "base {index}\nanchor {index}").unwrap();
        writeln!(head, "head {index}\nanchor {index}").unwrap();
        writeln!(worktree, "worktree {index}\nanchor {index}").unwrap();
    }

    let stack = align(&[&base, &head, &worktree]).unwrap();
    assert_eq!(stack.lines().len(), 20_000);
    assert_eq!(stack.hunks(0, None).len(), 5_000);
    assert_eq!(stack.lines().last().unwrap().line_number(2), Some(10_000));
}

#[test]
fn aligns_a_long_history_without_revisiting_dead_lines() {
    let versions: Vec<_> = ["even", "odd"].into_iter().cycle().take(10_000).collect();
    let stack = align(&versions).unwrap();
    assert_eq!(stack.lines().len(), versions.len());
    for (version, line) in stack.lines().iter().enumerate() {
        assert_eq!(line.born(), version);
        assert_eq!(line.line_number(version), Some(1));
        assert_eq!(
            line.dies(),
            (version + 1 < versions.len()).then_some(version + 1)
        );
    }
}

fn version_text() -> impl Strategy<Value = String> {
    let alphabet = prop::sample::select(vec!["", "a", "b", "    a", "\t{", "}", "é", "a\r", "\r"]);
    (prop::collection::vec(alphabet, 0..18), any::<bool>()).prop_map(|(lines, newline)| {
        let mut text = lines.join("\n");
        if newline && !lines.is_empty() {
            text.push('\n');
        }
        text
    })
}

fn split_expected(text: &str) -> Vec<&str> {
    let mut lines: Vec<_> = text.split('\n').collect();
    if text.is_empty() || text.ends_with('\n') {
        lines.pop();
    }
    lines
}

proptest! {
    #[test]
    fn preserves_every_version_and_covers_every_change(
        versions in prop::collection::vec(version_text(), 1..=5),
        context in 0_usize..=6,
        transition_range in (1_usize..6, 1_usize..7),
    ) {
        let inputs: Vec<_> = versions.iter().map(String::as_str).collect();
        let expected: Vec<_> = inputs.iter().map(|text| split_expected(text)).collect();
        let stack = align(&inputs).unwrap();
        prop_assert_eq!(stack.version_count(), versions.len());

        for line in stack.lines() {
            prop_assert!(line.born() < versions.len());
            if let Some(dies) = line.dies() {
                prop_assert!(line.born() < dies && dies < versions.len());
            }
            prop_assert_eq!(line.line_number(versions.len()), None);
        }

        for (version, expected_lines) in expected.iter().enumerate() {
            let mut num_alive = 0;
            for line in stack.lines() {
                let alive = line.born() <= version && line.dies().is_none_or(|dies| version < dies);
                if alive {
                    let Some(&raw) = expected_lines.get(num_alive) else {
                        return Err(TestCaseError::fail("too many live display lines"));
                    };
                    num_alive += 1;
                    let number = u32::try_from(num_alive).unwrap();
                    prop_assert_eq!(line.line_number(version), Some(number));
                    prop_assert_eq!(line.text(), raw.strip_suffix('\r').unwrap_or(raw));

                    let birth_number = line.line_number(line.born()).unwrap() as usize;
                    prop_assert_eq!(expected[line.born()][birth_number - 1], raw);
                } else {
                    prop_assert_eq!(line.line_number(version), None);
                }
            }
            prop_assert_eq!(num_alive, expected_lines.len());
        }

        let (start, end) = transition_range;
        for selected in [None, Some(start..end)] {
            let hunks = stack.hunks(context, selected.clone());
            prop_assert!(hunks.windows(2).all(|pair| pair[0].end < pair[1].start));
            prop_assert!(hunks.iter().all(|hunk| hunk.start < hunk.end && hunk.end <= stack.lines().len()));

            let mut changed = vec![false; stack.lines().len()];
            for version in 1..versions.len() {
                if selected.as_ref().is_none_or(|range| range.contains(&version)) {
                    for (line, changed) in stack.lines().iter().zip(&mut changed) {
                        *changed |= line.line_number(version - 1).is_some() != line.line_number(version).is_some();
                    }
                    if stack.trailing_newline_changed(version) == Some(true) {
                        let last_alive = |at| stack.lines().iter().rposition(|line| line.line_number(at).is_some());
                        if let Some(index) = last_alive(version).or_else(|| last_alive(version - 1)) {
                            changed[index] = true;
                        }
                    }
                }
            }

            for index in 0..stack.lines().len() {
                let expected_visible = changed.iter().enumerate().any(|(other, &is_changed)| {
                    is_changed && index.abs_diff(other) <= context
                });
                let actual_visible = hunks.iter().any(|hunk| hunk.contains(&index));
                prop_assert_eq!(actual_visible, expected_visible);
            }
        }
    }
}
