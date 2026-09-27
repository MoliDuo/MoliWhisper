//! What the user binds: a bare modifier or a key combination, and a mode.

use serde::{Deserialize, Serialize};

/// A modifier key, told apart by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModKey {
    LeftCtrl,
    RightCtrl,
    LeftShift,
    RightShift,
    LeftAlt,
    RightAlt,
    LeftMeta,
    RightMeta,
    Fn,
}

impl ModKey {
    /// The side-agnostic flag this key sets.
    pub fn mods(self) -> Mods {
        use ModKey::*;
        let mut m = Mods::default();
        match self {
            LeftCtrl | RightCtrl => m.ctrl = true,
            LeftShift | RightShift => m.shift = true,
            LeftAlt | RightAlt => m.alt = true,
            LeftMeta | RightMeta => m.meta = true,
            Fn => {}
        }
        m
    }

    pub fn label(self, mac: bool) -> &'static str {
        use ModKey::*;
        match (self, mac) {
            (LeftCtrl, true) => "左 Control",
            (RightCtrl, true) => "右 Control",
            (LeftShift, true) => "左 Shift",
            (RightShift, true) => "右 Shift",
            (LeftAlt, true) => "左 Option",
            (RightAlt, true) => "右 Option",
            (LeftMeta, true) => "左 Command",
            (RightMeta, true) => "右 Command",
            (Fn, true) => "Fn",
            (LeftCtrl, false) => "左 Ctrl",
            (RightCtrl, false) => "右 Ctrl",
            (LeftShift, false) => "左 Shift",
            (RightShift, false) => "右 Shift",
            (LeftAlt, false) => "左 Alt",
            (RightAlt, false) => "右 Alt",
            (LeftMeta, false) => "左 Win",
            (RightMeta, false) => "右 Win",
            (Fn, false) => "Fn",
        }
    }
}

/// Held modifiers, ignoring side.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub fn without(self, other: Mods) -> Mods {
        Mods {
            ctrl: self.ctrl && !other.ctrl,
            shift: self.shift && !other.shift,
            alt: self.alt && !other.alt,
            meta: self.meta && !other.meta,
        }
    }

    pub fn label(self, mac: bool) -> String {
        let mut s = String::new();
        if mac {
            for (on, sym) in [
                (self.ctrl, "⌃"),
                (self.alt, "⌥"),
                (self.shift, "⇧"),
                (self.meta, "⌘"),
            ] {
                if on {
                    s.push_str(sym);
                }
            }
        } else {
            for (on, name) in [
                (self.ctrl, "Ctrl+"),
                (self.meta, "Win+"),
                (self.alt, "Alt+"),
                (self.shift, "Shift+"),
            ] {
                if on {
                    s.push_str(name);
                }
            }
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hotkey {
    /// A modifier pressed on its own.
    Modifier { key: ModKey },
    /// A key pressed with exactly these modifiers held. `code` is the
    /// platform key code (a macOS virtual key code or a Windows VK).
    Combo { mods: Mods, code: u32 },
}

impl Default for Hotkey {
    fn default() -> Self {
        Hotkey::Modifier {
            key: ModKey::RightAlt,
        }
    }
}

impl Hotkey {
    /// Text for the settings page; `code_label` names a platform key code.
    pub fn label(&self, mac: bool, code_label: impl Fn(u32) -> String) -> String {
        match *self {
            Hotkey::Modifier { key } => key.label(mac).to_string(),
            Hotkey::Combo { mods, code } => format!("{}{}", mods.label(mac), code_label(code)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Press once to start, again to stop.
    #[default]
    Toggle,
    /// Hold to talk, release to stop.
    PushToTalk,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_readably() {
        let hk = Hotkey::Combo {
            mods: Mods {
                meta: true,
                shift: true,
                ..Mods::default()
            },
            code: 49,
        };
        let json = serde_json::to_string(&hk).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"combo","mods":{"ctrl":false,"shift":true,"alt":false,"meta":true},"code":49}"#
        );
        assert_eq!(serde_json::from_str::<Hotkey>(&json).unwrap(), hk);
        let bare: Hotkey =
            serde_json::from_str(r#"{"kind":"modifier","key":"right_alt"}"#).unwrap();
        assert_eq!(bare, Hotkey::default());
    }

    #[test]
    fn labels() {
        let hk = Hotkey::Combo {
            mods: Mods {
                meta: true,
                shift: true,
                ..Mods::default()
            },
            code: 49,
        };
        assert_eq!(hk.label(true, |_| "Space".into()), "⇧⌘Space");
        assert_eq!(hk.label(false, |_| "Space".into()), "Win+Shift+Space");
        assert_eq!(
            Hotkey::default().label(true, |_| unreachable!()),
            "右 Option"
        );
    }
}
