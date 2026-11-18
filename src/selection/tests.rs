use rune::{Cursor, Env, Key, KeyEvent, Mods, Runtime, clipboard, text::TextSystem};

use crate::{desktop::FONT_FAMILIES, state::State, view};

#[test]
fn dragging_code_keeps_the_ibeam_and_copies_without_a_view_commit() {
    let mut rt = Runtime::with_text(
        State::open(None, true, false),
        view::review,
        (1180.0, 720.0),
        Env::new(true, 2.0),
        TextSystem::new(&[], FONT_FAMILIES),
    );
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    // Count the commits production would make, without Rune's debug stale-view audits.
    rt.check_live_only(false);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let code = rt
        .elements()
        .into_iter()
        .find(|node| {
            node.key.as_deref() == Some("code")
                && node.text.as_deref() == Some("# Review shortcuts")
        })
        .unwrap();
    let [left, top, _, height] = code.rect;
    let y = top + height / 2.0;
    rt.pointer_move(left - 35.0, y);
    assert_eq!(rt.cursor(), Cursor::Default);
    rt.pointer_move(left + 1.0, y);
    assert_eq!(rt.cursor(), Cursor::Text);
    rt.pointer_down(left + 1.0, y);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let commits = rt.stats.commits;
    for x in [50.0, 100.0, 150.0, 200.0] {
        rt.pointer_move(left + x, y);
        assert_eq!(rt.cursor(), Cursor::Text);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.stats.commits, commits);
    rt.pointer_up(left + 200.0, y);
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(board.content().unwrap().text(), "# Review shortcuts");
}
