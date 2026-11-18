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
fn repository_selection_restores_the_document_and_routes_validation_to_its_owner() {
    use crate::{
        preferences::Preferences,
        repositories::{Request, Side},
    };

    let (send, receive) = mpsc::channel();
    let mut state = State::with_workspace(Workspace::live(send));
    let mut tx = Tx::default();
    state.restore_workspace(
        Preferences {
            repositories: vec![("/first".into(), None), ("/second".into(), None)],
            selected: None,
            collapsed: false,
        },
        false,
        &mut tx,
    );
    for entry in &mut state.workspace.as_mut().unwrap().entries {
        entry.document = Some(Box::new(State::new(review(None), ready(), None)));
    }

    state.select_repository(0, &mut tx);
    state.set_filter(Filter::Committed, &mut tx);
    state.toggle_wrap(&mut tx);
    state.toggle_file(0, &mut tx);
    state.jump_to_file(1, &mut tx);
    let mut position = state.scroll_position.get();
    position.offset = [0.0, 85.0];
    state.scroll_position.set(position);
    let old_review = Arc::clone(&state.review);
    state.choose_revision(Side::Head, Some("another-head".into()), &mut tx);
    let choice = receive
        .try_iter()
        .find_map(|request| match request {
            Request::Choose(choice) => Some(choice),
            _ => None,
        })
        .unwrap();
    state.finish_choice(choice, Ok(()), &mut tx);
    state.choose_revision(Side::Base, Some("another-base".into()), &mut tx);
    let pending = receive
        .try_iter()
        .find_map(|request| match request {
            Request::Choose(choice) => Some(choice),
            _ => None,
        })
        .unwrap();

    state.select_repository(1, &mut tx);
    state.finish_choice(pending, Ok(()), &mut tx);
    assert_eq!(
        state
            .workspace
            .as_ref()
            .unwrap()
            .active()
            .unwrap()
            .repository
            .base,
        None
    );
    assert_eq!(state.filter, Filter::All);
    assert!(!state.wrap);
    state.select_repository(0, &mut tx);
    assert!(Arc::ptr_eq(&state.review, &old_review));
    assert_eq!(state.filter, Filter::Committed);
    assert!(state.wrap);
    assert_eq!(state.collapsed, HashSet::from([0]));
    assert_eq!(state.active_file, Some(1));
    assert_eq!(
        state
            .workspace
            .as_ref()
            .unwrap()
            .active()
            .unwrap()
            .repository
            .base
            .as_deref(),
        Some(std::ffi::OsStr::new("another-base"))
    );
    assert_eq!(
        state.scroll_position.get().offset.map(f32::to_bits),
        [0.0_f32.to_bits(), 85.0_f32.to_bits()]
    );
    assert_eq!(
        state
            .workspace
            .as_ref()
            .unwrap()
            .active()
            .unwrap()
            .head
            .as_deref(),
        Some(std::ffi::OsStr::new("another-head"))
    );
}

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

#[test]
fn pulses_only_on_same_comparison_reloads_and_clears_on_filter_change() {
    let input = crate::demo::review();
    let edited = crate::demo::edited(input.clone());
    let mut state = State::new(Review::default(), Status::Loading, None);
    let mut tx = Tx::default();
    state.replace_review(
        Replacement::new(Review::new(input.clone()), &state.review),
        &mut tx,
    );
    assert!(state.pulse.files().next().is_none());

    state.set_filter(Filter::Uncommitted, &mut tx);
    assert!(state.pulse.files().next().is_none());
    state.replace_review(
        Replacement::new(Review::new(edited.clone()), &state.review),
        &mut tx,
    );
    assert!(state.pulse.files().next().is_some());

    state.set_filter(Filter::All, &mut tx);
    assert!(state.pulse.files().next().is_none());
    let mut edited_again = edited.clone();
    edited_again.files[0]
        .worktree
        .as_mut()
        .unwrap()
        .text
        .as_mut()
        .unwrap()
        .push_str("// Another update.\n");
    state.replace_review(
        Replacement::new(Review::new(edited_again), &state.review),
        &mut tx,
    );
    assert!(state.pulse.files().next().is_some());

    let other_base = ReviewInput {
        base_revision: [Some("another-base-tip".into()), None],
        ..edited
    };
    state.replace_review(
        Replacement::new(Review::new(other_base), &state.review),
        &mut tx,
    );
    assert!(state.pulse.files().next().is_none());
    assert!(state.pulse.opacity().get() < f32::EPSILON);
}

#[test]
fn reduced_motion_pulse_fades_in_place_and_goes_idle() {
    let mut input = crate::demo::review();
    input.files.truncate(1);
    let state = State::new(Review::new(input.clone()), ready(), None);
    let mut runtime = rune::Runtime::new(
        state,
        crate::view::review,
        (1240.0, 820.0),
        rune::Env::new(false, 1.0),
    );
    runtime.set_system_reduced_motion(true);
    runtime.poll = Some(State::poll);
    runtime.frame(0.0);

    runtime.update(|state, tx| {
        let next = Review::new(crate::demo::edited(input));
        state.replace_review(Replacement::new(next, &state.review), tx);
    });
    runtime.frame(1.0 / 60.0);
    let wash = |rt: &rune::Runtime<State>| {
        rt.elements()
            .into_iter()
            .find(|element| {
                element.key.as_deref() == Some("pulse-tint") && element.path.contains("/diff/")
            })
            .unwrap()
    };
    let first = wash(&runtime);
    runtime.frame(0.6);
    let later = wash(&runtime);
    assert!(first.opacity > 0.95);
    assert!(later.opacity > 0.1 && later.opacity < first.opacity);
    assert!(
        first
            .rect
            .iter()
            .zip(later.rect)
            .all(|(a, b)| (a - b).abs() < 0.01)
    );

    runtime.frame(0.7);
    assert!(runtime.state.pulse.files().next().is_none());
    assert!(
        !runtime
            .elements()
            .iter()
            .any(|element| element.key.as_deref() == Some("pulse-tint"))
    );
    assert!(!runtime.frame(1.0 / 60.0));
    assert!(runtime.wake_deadline().is_none());
}

#[test]
fn offscreen_pulse_jumps_to_the_closest_edit() {
    use rune::{Env, Mods, Runtime, motion_gate::Run};

    let (_directory, repository) = crate::source::tests::fixture();
    let settle = |runtime: &mut Runtime<State>, frames| {
        for _ in 0..frames {
            runtime.frame(1.0 / 60.0);
        }
    };
    for (start, chip) in [
        ("file/src/lib.rs", "pulse-offscreen/above"),
        ("file/src/config.rs", "pulse-offscreen/below"),
    ] {
        let state = State::open(Some(repository.clone()), true, true);
        let mut runtime = Runtime::new(
            state,
            crate::view::review,
            (1400.0, 900.0),
            Env::new(true, 1.0),
        );
        runtime.on_key = Some(State::key);
        runtime.poll = Some(State::poll);
        settle(&mut runtime, 12);
        Run::tap(&mut runtime, start);
        settle(&mut runtime, 30);
        assert!(runtime.find(chip).is_some());

        if chip.ends_with("above") {
            Run::tap(&mut runtime, chip);
        } else {
            runtime.key(KeyEvent {
                key: Key::Text("n".into()),
                mods: Mods::default(),
            });
        }
        settle(&mut runtime, 8);

        let file = runtime.state.active_file.unwrap();
        let generated = &runtime.state.review.files[file];
        assert_eq!(generated.path(), "src/generated.rs");
        let FileBody::Text(diff) = generated.body(Filter::All) else {
            unreachable!("the generated fixture is text");
        };
        let line = diff
            .lines
            .iter()
            .position(|line| line.worktree == Some(4200))
            .unwrap();
        let elements = runtime.elements();
        let top = |key: &str| {
            elements
                .iter()
                .find(|element| element.key.as_deref() == Some(key))
                .unwrap()
                .rect[1]
        };
        let row_top = top(&format!("line/{file}/{line}"));
        assert!((row_top - top("diff-scroll") - HEADER_HEIGHT).abs() < 1.0);
        assert_eq!(runtime.state.pulse_offscreen(), (0, 0));
        assert!(runtime.find("pulse-offscreen").is_none());
    }
}

#[test]
fn filter_motion_gate() {
    use rune::{Env, Runtime, motion_gate::Run};

    for reduced in [false, true] {
        let mut input = crate::demo::review();
        input.files.truncate(1);
        let mut state = State::new(Review::new(input), ready(), None);
        if reduced {
            state.force_reduced_motion();
        }
        let runtime = Runtime::new(
            state,
            crate::view::review,
            (1240.0, 820.0),
            Env::new(true, 1.0),
        );
        let mut run = Run::new(runtime, 60.0);
        run.idle(0.2);
        run.step();
        let lines = |runtime: &Runtime<State>| {
            runtime
                .elements()
                .into_iter()
                .filter(|element| {
                    !element.exiting
                        && element.visible[3] > 0.0
                        && element
                            .key
                            .as_deref()
                            .is_some_and(|key| key.starts_with("line/"))
                })
                .collect::<Vec<_>>()
        };
        let before = lines(&run.rt);
        let head_only = |element: &rune::ElementInfo| {
            element.path.contains("/gutter/new-number/")
                && element
                    .text
                    .as_deref()
                    .is_some_and(|text| text.starts_with('H'))
                && element.opacity > 0.01
                && element.visible[3] > 0.0
        };
        assert!(run.rt.elements().iter().any(head_only));
        run.act("tap Committed", |rt| Run::tap(rt, "filter/committed"));
        assert!(!run.rt.elements().iter().any(head_only));
        run.play(0.8);
        run.quiet();
        run.play(0.15);
        let after = lines(&run.rt);
        assert_eq!(run.rt.state.filter, Filter::Committed);
        let moved = before
            .iter()
            .filter_map(|old| {
                let new = after.iter().find(|new| new.path == old.path)?;
                ((old.rect[1] - new.rect[1]).abs() > 10.0).then_some((old, new))
            })
            .collect::<Vec<_>>();
        assert!(
            !moved.is_empty(),
            "the fixture must move some of the retained rows"
        );
        let glides = moved.iter().any(|(old, new)| {
            let min = old.rect[1].min(new.rect[1]) + 1.0;
            let max = old.rect[1].max(new.rect[1]) - 1.0;
            run.rec.tracks.iter().any(|track| {
                track.path == old.path
                    && track
                        .frames
                        .iter()
                        .any(|(_, frame)| (min..max).contains(&frame.rect[1]))
            })
        });
        assert_eq!(
            glides, !reduced,
            "retained rows should glide only with motion enabled"
        );
        if !reduced {
            let findings = run.finish();
            assert!(findings.is_empty(), "{findings:#?}");
        }
    }
}

#[test]
fn filter_switches_keep_a_selected_file_header_at_the_top() {
    use rune::{Env, Runtime, motion_gate::Run};

    let (_directory, repository) = crate::source::tests::fixture();
    let state = State::open(Some(repository), true, false);
    let file = state
        .review
        .files
        .iter()
        .position(|file| file.path() == "src/lib.rs")
        .unwrap();
    let key = format!("header/{file}");
    let mut runtime = Runtime::new(
        state,
        crate::view::review,
        (1400.0, 900.0),
        Env::new(true, 1.0),
    );
    runtime.frame(0.0);
    runtime.update(|state, tx| state.jump_to_file(file, tx));
    let settle = |runtime: &mut Runtime<State>| {
        for _ in 0..48 {
            runtime.frame(1.0 / 60.0);
        }
    };
    settle(&mut runtime);
    let (_, top) = runtime.find(&key).unwrap();
    for filter in ["filter/committed", "filter/uncommitted", "filter/all"] {
        Run::tap(&mut runtime, filter);
        settle(&mut runtime);
        let (_, y) = runtime.find(&key).unwrap();
        assert!(
            (y - top).abs() < 1.0,
            "{filter} moved the selected header to {y}"
        );
        assert_eq!(runtime.state.active_file, Some(file));
    }
}
