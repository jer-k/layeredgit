use std::{
    error::Error,
    fs,
    path::Path,
    process,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use super::*;
use crate::Emulator;

const SIZE: Size = Size {
    cols: 80,
    rows: 11,
    cell_width: 8,
    cell_height: 20,
};
const LARGE: Size = Size {
    cols: 210,
    rows: 53,
    ..SIZE
};

#[test]
#[ignore = "requires the running local Rex server and creates an isolated test session"]
fn owns_resizes_and_reattaches_to_a_live_rex_shell() -> Result<(), Box<dyn Error>> {
    let label = format!(
        "layeredgit-test-{}-{}",
        process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    );
    let cwd = std::env::temp_dir().join(&label);
    fs::create_dir(&cwd)?;
    let result = live_test(&cwd, &label);
    let remove = fs::remove_dir(&cwd);
    result?;
    remove?;
    Ok(())
}

fn live_test(cwd: &Path, label: &str) -> Result<(), Box<dyn Error>> {
    let mut control = Control::connect(&socket::endpoint()?)?;
    let target = Target::discover(&mut control, cwd, SIZE, &AtomicBool::new(false))?;
    let result = exercise_shell(cwd, label, &mut control, &target);
    control
        .call::<serde_json::Value>("session.destroy", &json!({"session_id": target.session_id}))?;
    result
}

fn take(control: &mut Control, target: &Target) -> io::Result<serde_json::Value> {
    let reply: serde_json::Value = control.call(
        "session.own_block",
        &json!({
            "session_id": target.session_id,
            "block_ids": [target.block_id],
            "mode": "take",
        }),
    )?;
    Ok(reply["blocks"][0]["owner"].clone())
}

fn size(control: &mut Control, target: &Target) -> io::Result<serde_json::Value> {
    target.terminal(control, "com.superlogical.terminal.size", &json!({}))
}

fn check_size(actual: &serde_json::Value, owner: &serde_json::Value, grid: Size) -> io::Result<()> {
    if owner.is_string()
        && actual["owner"] == *owner
        && actual["columns"] == grid.cols
        && actual["rows"] == grid.rows
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Expected {owner} to own a {}x{} terminal, got {actual}",
            grid.cols, grid.rows
        )))
    }
}

fn verify_ownership_api(control: &mut Control, target: &Target) -> io::Result<()> {
    let first = take(control, target)?;
    target.terminal(
        control,
        "com.superlogical.terminal.resize",
        &size_args(LARGE),
    )?;

    let mut other = Control::connect(&socket::endpoint()?)?;
    other.call::<serde_json::Value>("session.attach", &json!({"session_id": target.session_id}))?;
    target.terminal(
        &mut other,
        "com.superlogical.terminal.resize",
        &size_args(SIZE),
    )?;
    let block_event = |other: &mut Control, event| {
        other.call::<serde_json::Value>(
            "block.event",
            &json!({
                "session_id": target.session_id, "block_id": target.block_id, "event": event,
            }),
        )
    };
    target.focus_event(&mut other, true)?;
    block_event(
        &mut other,
        json!({"type": "key", "payload": {
            "key": "key_u", "mods": "ctrl", "consumed_mods": "", "text": "",
            "unshifted_codepoint": 117, "composing": false, "action": "press",
        }}),
    )?;
    check_size(&size(control, target)?, &first, LARGE)?;

    let second = take(&mut other, target)?;
    if first == second {
        return Err(io::Error::other(
            "Rex did not transfer ownership to the second client",
        ));
    }
    check_size(&size(control, target)?, &second, SIZE)
}

fn exercise_shell(
    cwd: &Path,
    label: &str,
    control: &mut Control,
    target: &Target,
) -> Result<(), Box<dyn Error>> {
    verify_ownership_api(control, target)?;
    // Simulate another app owning the shared block with a different grid.
    let foreign_owner = take(control, target)?;
    let source = RexPty::connect(cwd.to_owned(), SIZE, Arc::new(|| {}))?;
    let snapshot = attached(&source, label)?;
    let mut emulator = Emulator::from_snapshot(&snapshot)?;
    apply_resize(&source, &mut emulator, LARGE)?;
    check_size(&size(control, target)?, &foreign_owner, LARGE)?;

    source.focus(true);
    apply_resize(&source, &mut emulator, SIZE)?;
    let focused = size(control, target)?;
    let pane_owner = &focused["owner"];
    if *pane_owner == foreign_owner {
        return Err(io::Error::other("Rex pane did not take ownership on focus").into());
    }
    check_size(&focused, pane_owner, SIZE)?;

    take(control, target)?;
    apply_resize(&source, &mut emulator, LARGE)?;
    source.write(b"echo hi; stty size\r".to_vec());
    apply_resize(&source, &mut emulator, SIZE)?;
    wait_for_output(&source, b"\r\nhi\r\n11 80\r\n")?;
    check_size(&size(control, target)?, pane_owner, SIZE)?;

    let resized = Size {
        cols: 70,
        rows: 9,
        ..SIZE
    };
    source.resize(resized);
    apply_resize(&source, &mut emulator, resized)?;
    check_size(&size(control, target)?, pane_owner, resized)?;
    drop(source);

    let source = RexPty::connect(cwd.to_owned(), SIZE, Arc::new(|| {}))?;
    let snapshot = attached(&source, label)?;
    let mut emulator = Emulator::from_snapshot(&snapshot)?;
    emulator.update()?;
    let restored = emulator.screen().rows.iter().flatten().any(|row| {
        let text: String = row.cells.iter().map(|cell| cell.text.as_str()).collect();
        text.trim() == "hi"
    });
    if !restored {
        return Err(io::Error::other("Rex snapshot did not restore the output of echo hi").into());
    }
    Ok(())
}

fn apply_resize(source: &RexPty, emulator: &mut Emulator, expected: Size) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let SourceEvent::Resized(size) = event(source, deadline)?
            && size == expected
        {
            emulator.resize(size).map_err(io::Error::other)?;
            emulator.update().map_err(io::Error::other)?;
            if emulator.screen().rows.len() == usize::from(size.rows) {
                return Ok(());
            }
            return Err(io::Error::other(
                "Ghostty did not adopt the effective Rex size",
            ));
        }
    }
}

fn wait_for_output(source: &RexPty, expected: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut bytes = Vec::new();
    loop {
        if let SourceEvent::Output(chunk) = event(source, deadline)? {
            bytes.extend(chunk);
            if bytes
                .windows(expected.len())
                .any(|window| window == expected)
            {
                return Ok(());
            }
        }
    }
}

fn attached(source: &RexPty, label: &str) -> io::Result<Vec<u8>> {
    match event(source, Instant::now() + Duration::from_secs(8))? {
        SourceEvent::RexAttached {
            session_label,
            snapshot,
        } if session_label == label => Ok(snapshot),
        event => Err(io::Error::other(format!(
            "Expected attachment to {label}, got {event:?}"
        ))),
    }
}

fn event(source: &RexPty, deadline: Instant) -> io::Result<SourceEvent> {
    let event = source
        .events
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(io::Error::other)?;
    match event {
        SourceEvent::Failed(error) | SourceEvent::Exited(error) => Err(io::Error::other(error)),
        event => Ok(event),
    }
}
