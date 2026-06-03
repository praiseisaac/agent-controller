//! ASCII → USB HID keyboard usage codes (page 0x07), used to synthesize typing
//! via `idb`'s `hid` stream.

/// Left Shift usage code.
pub const SHIFT: u64 = 225;

/// Map a character to its `(usage_code, needs_shift)`, if typable.
pub fn char_to_hid(c: char) -> Option<(u64, bool)> {
    Some(match c {
        'a'..='z' => (4 + (c as u64 - 'a' as u64), false),
        'A'..='Z' => (4 + (c as u64 - 'A' as u64), true),
        '1'..='9' => (30 + (c as u64 - '1' as u64), false),
        '0' => (39, false),
        ' ' => (44, false),
        '\n' | '\r' => (40, false),
        '\t' => (43, false),
        '!' => (30, true),
        '@' => (31, true),
        '#' => (32, true),
        '$' => (33, true),
        '%' => (34, true),
        '^' => (35, true),
        '&' => (36, true),
        '*' => (37, true),
        '(' => (38, true),
        ')' => (39, true),
        '-' => (45, false),
        '_' => (45, true),
        '=' => (46, false),
        '+' => (46, true),
        '[' => (47, false),
        '{' => (47, true),
        ']' => (48, false),
        '}' => (48, true),
        '\\' => (49, false),
        '|' => (49, true),
        ';' => (51, false),
        ':' => (51, true),
        '\'' => (52, false),
        '"' => (52, true),
        '`' => (53, false),
        '~' => (53, true),
        ',' => (54, false),
        '<' => (54, true),
        '.' => (55, false),
        '>' => (55, true),
        '/' => (56, false),
        '?' => (56, true),
        _ => return None,
    })
}

/// A named key (for `press`) → its HID usage code, if it maps to the keyboard.
pub fn named_key(name: &str) -> Option<u64> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => 40,
        "escape" | "esc" => 41,
        "backspace" | "delete" => 42,
        "tab" => 43,
        "space" => 44,
        "right" => 79,
        "left" => 80,
        "down" => 81,
        "up" => 82,
        _ => return None,
    })
}
