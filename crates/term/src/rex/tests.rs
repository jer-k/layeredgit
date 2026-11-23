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

#[test]
#[ignore = "requires the running local Rex server and creates an isolated test session"]
fn writes_and_reattaches_to_a_live_rex_shell() -> Result<(), Box<dyn Error>> {
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

fn exercise_shell(
    cwd: &Path,
    label: &str,
    control: &mut Control,
    target: &Target,
) -> Result<(), Box<dyn Error>> {
    // Simulate another app owning the shared block with a different grid.
    control.call::<serde_json::Value>(
        "session.own_block",
        &json!({
            "session_id": target.session_id,
            "block_ids": [target.block_id],
            "mode": "take",
        }),
    )?;
    let owned = Size { cols: 72, ..SIZE };
    target.terminal(
        control,
        "com.superlogical.terminal.resize",
        &size_args(owned),
    )?;
    let source = RexPty::connect(cwd.to_owned(), SIZE, Arc::new(|| {}))?;
    attached(&source, label)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    if !matches!(event(&source, deadline)?, SourceEvent::Resized(size) if size == owned) {
        return Err(io::Error::other("Rex did not report its owner's size on attach").into());
    }
    source.resize(SIZE);
    source.write(b"echo hi\r".to_vec());
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut bytes = Vec::new();
    loop {
        if let SourceEvent::Output(chunk) = event(&source, deadline)? {
            bytes.extend(chunk);
            if bytes
                .windows(b"\r\nhi\r\n".len())
                .any(|window| window == b"\r\nhi\r\n")
            {
                break;
            }
        }
    }
    let size = target.terminal(control, "com.superlogical.terminal.size", &json!({}))?;
    if size["columns"] != owned.cols {
        return Err(
            io::Error::other("The embedded pane overrode another Rex client's size").into(),
        );
    }
    let resized = Size {
        cols: 70,
        rows: 9,
        ..SIZE
    };
    target.terminal(
        control,
        "com.superlogical.terminal.resize",
        &size_args(resized),
    )?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if matches!(event(&source, deadline)?, SourceEvent::Resized(size) if size == resized) {
            break;
        }
    }
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
