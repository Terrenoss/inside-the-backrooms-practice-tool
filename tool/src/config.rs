// Hotkeys, read from backrooms_practice_tool.cfg next to the DLL ("action = KEY" lines).
use std::path::Path;

/// (action, default key). The index is the action id used by the overlay.
pub const ACTIONS: [(&str, &str); 14] = [
    ("menu", "F1"),
    ("noclip", "F2"),
    ("god", "F3"),
    ("entites", "F4"),
    ("sauver", "F5"),
    ("charger", "F6"),
    ("effets_off", "F7"),
    ("colliders", "F8"),
    ("triggers", "F9"),
    ("stamina", ""),
    ("items_infinis", ""),
    ("anti_chute_off", ""),
    ("slot_precedent", ""),
    ("slot_suivant", ""),
];

pub type Keys = [Option<u16>; ACTIONS.len()];

/// Windows virtual-key code for a key name (F1-F12, A-Z, 0-9, NUMPAD0-9, INSERT, ...).
pub fn key_code(name: &str) -> Option<u16> {
    let n = name.trim().to_ascii_uppercase();
    let named = [("INSERT", 0x2D), ("DELETE", 0x2E), ("HOME", 0x24), ("END", 0x23), ("PAGEUP", 0x21), ("PAGEDOWN", 0x22)];
    if let Some(&(_, vk)) = named.iter().find(|(k, _)| *k == n) {
        return Some(vk);
    }
    if let Some(i) = n.strip_prefix("NUMPAD").and_then(|d| d.parse::<u16>().ok()).filter(|d| *d <= 9) {
        return Some(0x60 + i);
    }
    if let Some(i) = n.strip_prefix('F').and_then(|d| d.parse::<u16>().ok()).filter(|d| (1..=12).contains(d)) {
        return Some(0x6F + i);
    }
    match n.as_bytes() {
        [c @ (b'A'..=b'Z' | b'0'..=b'9')] => Some(*c as u16),
        _ => None,
    }
}

pub fn key_name(vk: u16) -> String {
    match vk {
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("NUMPAD{}", vk - 0x60),
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x2D => "INSERT".into(),
        0x2E => "DELETE".into(),
        0x24 => "HOME".into(),
        0x23 => "END".into(),
        0x21 => "PAGEUP".into(),
        0x22 => "PAGEDOWN".into(),
        _ => format!("0x{vk:X}"),
    }
}

/// Defaults, overridden by every recognized "action = KEY" line. An empty value unbinds.
pub fn parse(text: &str) -> Keys {
    let mut keys: Keys = ACTIONS.map(|(_, k)| key_code(k));
    for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let Some((action, key)) = line.split_once('=') else { continue };
        if let Some(i) = ACTIONS.iter().position(|(a, _)| *a == action.trim()) {
            keys[i] = key_code(key);
        }
    }
    keys
}

pub fn default_file() -> String {
    let mut s = String::from(
        "# Touches du practice tool (relance le jeu + l'injecteur apres modification).\n\
         # Noms possibles : F1-F12, A-Z, 0-9, NUMPAD0-NUMPAD9, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN.\n\
         # Laisser vide = pas de touche.\n",
    );
    for (a, k) in ACTIONS {
        s += &format!("{a} = {k}\n");
    }
    s
}

/// Reads the config, writing the default one first if it doesn't exist.
pub fn load(path: &Path) -> Keys {
    if !path.exists() {
        let _ = std::fs::write(path, default_file());
    }
    parse(&std::fs::read_to_string(path).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_overrides_and_defaults() {
        let keys = parse("# c\nmenu = insert\nnoclip =\nstamina = numpad5\ngod= g\nbogus = F1\n");
        assert_eq!(keys[0], Some(0x2D));
        assert_eq!(keys[1], None);
        assert_eq!(keys[2], Some(b'G' as u16));
        assert_eq!(keys[3], Some(0x73)); // entites keeps default F4
        assert_eq!(keys[9], Some(0x65));
        assert_eq!(parse(&default_file()), ACTIONS.map(|(_, k)| key_code(k)));
        for vk in [0x70, 0x7B, 0x60, 0x41, 0x30, 0x2D] {
            assert_eq!(key_code(&key_name(vk)), Some(vk));
        }
        assert_eq!(key_code("F13"), None);
    }
}
