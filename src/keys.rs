//! Key representation and parsing of config key strings like `ctrl+e`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A normalized key: uppercase-char keys absorb the SHIFT modifier, so
/// `shift+j`, `J`, and a crossterm event for shift-j all compare equal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }.normalized()
    }

    fn normalized(mut self) -> Self {
        if let KeyCode::Char(c) = self.code {
            if self.mods.contains(KeyModifiers::SHIFT) && c.is_alphabetic() {
                self.code = KeyCode::Char(c.to_ascii_uppercase());
            }
            // SHIFT carries no extra information for character keys.
            self.mods.remove(KeyModifiers::SHIFT);
        }
        self
    }

    /// Parse a config key string such as `j`, `J`, `ctrl+e`, `shift+right`,
    /// `ctrl+enter`, or `alt+s`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut mods = KeyModifiers::NONE;
        let parts: Vec<&str> = s.split('+').collect();
        let (mod_parts, key_part) = match parts.split_last() {
            Some((last, rest)) if !last.is_empty() => (rest, *last),
            _ => return Err(format!("invalid key: {s:?}")),
        };
        for part in mod_parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier: {other:?}")),
            }
        }
        let code = parse_key_name(key_part)?;
        Ok(Self::new(code, mods))
    }

    /// Normalize an incoming crossterm event into a `Key`.
    pub fn from_event(ev: KeyEvent) -> Self {
        Self::new(ev.code, ev.modifiers)
    }
}

fn parse_key_name(name: &str) -> Result<KeyCode, String> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(KeyCode::Char(c));
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "up" | "up-arrow" => KeyCode::Up,
        "down" | "down-arrow" => KeyCode::Down,
        "left" | "left-arrow" => KeyCode::Left,
        "right" | "right-arrow" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page-up" => KeyCode::PageUp,
        "pagedown" | "page-down" => KeyCode::PageDown,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        f if f.starts_with('f') => {
            let n: u8 = f[1..]
                .parse()
                .map_err(|_| format!("unknown key: {name:?}"))?;
            if (1..=24).contains(&n) {
                KeyCode::F(n)
            } else {
                return Err(format!("unknown key: {name:?}"));
            }
        }
        _ => return Err(format!("unknown key: {name:?}")),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> Key {
        Key::new(code, mods)
    }

    #[test]
    fn parses_bare_char() {
        assert_eq!(
            Key::parse("j").unwrap(),
            key(KeyCode::Char('j'), KeyModifiers::NONE)
        );
    }

    #[test]
    fn parses_ctrl_char() {
        assert_eq!(
            Key::parse("ctrl+e").unwrap(),
            key(KeyCode::Char('e'), KeyModifiers::CONTROL)
        );
    }

    #[test]
    fn parses_alt_char() {
        assert_eq!(
            Key::parse("alt+s").unwrap(),
            key(KeyCode::Char('s'), KeyModifiers::ALT)
        );
    }

    #[test]
    fn shift_letter_normalizes_to_uppercase_char() {
        // "shift+j" and "J" are the same key.
        assert_eq!(Key::parse("shift+j").unwrap(), Key::parse("J").unwrap());
        assert_eq!(
            Key::parse("J").unwrap(),
            key(KeyCode::Char('J'), KeyModifiers::NONE)
        );
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(
            Key::parse("enter").unwrap(),
            key(KeyCode::Enter, KeyModifiers::NONE)
        );
        assert_eq!(
            Key::parse("ctrl+enter").unwrap(),
            key(KeyCode::Enter, KeyModifiers::CONTROL)
        );
        assert_eq!(
            Key::parse("tab").unwrap(),
            key(KeyCode::Tab, KeyModifiers::NONE)
        );
        assert_eq!(
            Key::parse("shift+right").unwrap(),
            key(KeyCode::Right, KeyModifiers::SHIFT)
        );
        assert_eq!(
            Key::parse("esc").unwrap(),
            key(KeyCode::Esc, KeyModifiers::NONE)
        );
        assert_eq!(
            Key::parse("space").unwrap(),
            key(KeyCode::Char(' '), KeyModifiers::NONE)
        );
    }

    #[test]
    fn rejects_unknown_keys_and_modifiers() {
        assert!(Key::parse("bogus").is_err());
        assert!(Key::parse("hyper+j").is_err());
        assert!(Key::parse("").is_err());
    }

    #[test]
    fn event_normalization_matches_parse() {
        // Terminals report shift+j as Char('J') with SHIFT set.
        let ev = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(ev), Key::parse("J").unwrap());

        let ev = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(Key::from_event(ev), Key::parse("ctrl+e").unwrap());

        // SHIFT is preserved for non-char keys.
        let ev = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(Key::from_event(ev), Key::parse("shift+right").unwrap());
    }
}
