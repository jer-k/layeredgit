use rune::{Key, KeyEvent, Tx};
use term::{
    Emulator, VtError,
    key::{Action, Key as VtKey, Mods},
};

pub(super) fn encode(
    emulator: &mut Emulator,
    event: &KeyEvent,
    tx: &Tx,
    action: Action,
) -> Result<Vec<u8>, VtError> {
    let modifiers = event.mods;
    let mut mods = Mods::empty();
    mods.set(Mods::SHIFT, modifiers.shift);
    mods.set(Mods::CTRL, modifiers.ctrl);
    mods.set(Mods::ALT, modifiers.alt);
    mods.set(Mods::SUPER, modifiers.logo);
    let mut consumed = Mods::empty();
    let mut base = tx.key_base();
    let mut text = None;
    let key = match &event.key {
        Key::Text(value) => {
            let character = value.chars().next().filter(|_| value.chars().count() == 1);
            base = base.or_else(|| character.map(unshift));
            if !modifiers.ctrl && !modifiers.logo && action != Action::Release {
                text = Some(value.as_str());
                consumed.set(Mods::SHIFT, modifiers.shift);
                // On macOS Option keeps the character produced by the keyboard layout.
                consumed.set(Mods::ALT, modifiers.alt && cfg!(target_os = "macos"));
            }
            base.map_or(VtKey::Unidentified, printable)
        }
        Key::Command(value) => {
            base = base.or(Some(unshift(*value)));
            base.map_or(VtKey::Unidentified, printable)
        }
        Key::Enter => VtKey::Enter,
        Key::Tab => VtKey::Tab,
        Key::Backspace => VtKey::Backspace,
        Key::Delete => VtKey::Delete,
        Key::Escape => VtKey::Escape,
        Key::Left => VtKey::ArrowLeft,
        Key::Right => VtKey::ArrowRight,
        Key::Up => VtKey::ArrowUp,
        Key::Down => VtKey::ArrowDown,
        Key::Home => VtKey::Home,
        Key::End => VtKey::End,
        Key::PageUp => VtKey::PageUp,
        Key::PageDown => VtKey::PageDown,
        Key::Insert => VtKey::Insert,
        Key::Function(n @ 1..=24) => {
            VtKey::try_from(VtKey::F1 as i32 + i32::from(*n) - 1).unwrap_or(VtKey::Unidentified)
        }
        _ => return Ok(Vec::new()),
    };
    emulator.encode_key(key, action, mods, consumed, base, text)
}

fn unshift(value: char) -> char {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const UNMODIFIED: &str = "`1234567890-=[]\\;',./";
    SHIFTED
        .find(value)
        .and_then(|index| UNMODIFIED.as_bytes().get(index))
        .map_or_else(|| value.to_ascii_lowercase(), |&byte| char::from(byte))
}

fn printable(value: char) -> VtKey {
    match value {
        'a'..='z' => VtKey::try_from(VtKey::A as i32 + (value as i32 - 'a' as i32))
            .unwrap_or(VtKey::Unidentified),
        '0'..='9' => VtKey::try_from(VtKey::Digit0 as i32 + (value as i32 - '0' as i32))
            .unwrap_or(VtKey::Unidentified),
        '`' => VtKey::Backquote,
        '\\' => VtKey::Backslash,
        '[' => VtKey::BracketLeft,
        ']' => VtKey::BracketRight,
        ',' => VtKey::Comma,
        '=' => VtKey::Equal,
        '-' => VtKey::Minus,
        '.' => VtKey::Period,
        '\'' => VtKey::Quote,
        ';' => VtKey::Semicolon,
        '/' => VtKey::Slash,
        ' ' => VtKey::Space,
        _ => VtKey::Unidentified,
    }
}
