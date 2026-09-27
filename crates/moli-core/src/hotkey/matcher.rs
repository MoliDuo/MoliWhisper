//! Turns raw key events into dictation actions and swallow decisions.
//!
//! The platform hook calls [`Matcher::handle`] synchronously for every key
//! event and must return the verdict at once, so this does no IO and never
//! blocks. Events the app injected itself (the paste shortcut) are filtered
//! out by the hook before they get here.

use std::time::{Duration, Instant};

use super::spec::{Hotkey, ModKey, Mode, Mods};

/// A bare modifier held longer than this is not a tap (toggle mode).
pub const TAP_MAX: Duration = Duration::from_millis(600);
/// A push-to-talk press shorter than this cancels instead of stopping.
pub const HOLD_MIN: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Mod(ModKey),
    /// Any other key, by platform key code.
    Code(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub down: bool,
    /// Modifiers held according to the OS, including this key if it is one.
    pub mods: Mods,
    /// Auto-repeat, when the platform says so.
    pub repeat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Start,
    Stop,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Hotkey(Hotkey),
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Decision {
    /// Keep the event from reaching the focused app.
    pub swallow: bool,
    pub action: Option<Action>,
    /// Set when hotkey recording finished with this event.
    pub recorded: Option<Recorded>,
}

impl Decision {
    fn pass() -> Self {
        Self::default()
    }

    fn swallow() -> Self {
        Self {
            swallow: true,
            ..Self::default()
        }
    }

    fn act(action: Action) -> Self {
        Self {
            action: Some(action),
            ..Self::default()
        }
    }

    fn swallow_and(action: Action) -> Self {
        Self {
            swallow: true,
            action: Some(action),
            recorded: None,
        }
    }
}

/// A bare-modifier press in progress.
#[derive(Debug, Clone, Copy)]
struct Press {
    at: Instant,
    /// No other key was involved (toggle mode taps only count when clean).
    clean: bool,
    /// Push-to-talk: a session was started by this press.
    started: bool,
    /// Push-to-talk: the press already ended its session (cancelled).
    done: bool,
}

#[derive(Debug, Default)]
struct Recording {
    /// The modifier pressed first, while it may still become a bare hotkey.
    modifier: Option<ModKey>,
    dirty: bool,
}

pub struct Matcher {
    hotkey: Hotkey,
    mode: Mode,
    escape: u32,
    press: Option<Press>,
    /// When the combo went down (its down was swallowed).
    combo_at: Option<Instant>,
    /// Key codes whose down was swallowed; their up is swallowed too.
    swallowed: Vec<u32>,
    recording: Option<Recording>,
}

impl Matcher {
    /// `escape` is the platform key code of Escape.
    pub fn new(hotkey: Hotkey, mode: Mode, escape: u32) -> Self {
        Self {
            hotkey,
            mode,
            escape,
            press: None,
            combo_at: None,
            swallowed: Vec::new(),
            recording: None,
        }
    }

    pub fn hotkey(&self) -> Hotkey {
        self.hotkey
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set(&mut self, hotkey: Hotkey, mode: Mode) {
        self.hotkey = hotkey;
        self.mode = mode;
        self.press = None;
        self.combo_at = None;
    }

    /// The next hotkey the user presses is captured instead of acted on.
    pub fn start_recording(&mut self) {
        self.recording = Some(Recording::default());
        self.press = None;
        self.combo_at = None;
    }

    pub fn stop_recording(&mut self) {
        self.recording = None;
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// `active` is whether a session is connecting, recording or finalizing;
    /// Escape is only taken then.
    pub fn handle(&mut self, ev: &KeyEvent, now: Instant, active: bool) -> Decision {
        // The up of a swallowed down is always swallowed, whatever changed since.
        if let Key::Code(code) = ev.key {
            if !ev.down {
                if let Some(i) = self.swallowed.iter().position(|&c| c == code) {
                    self.swallowed.swap_remove(i);
                    return self.combo_up(code, now);
                }
            } else if self.swallowed.contains(&code) {
                return Decision::swallow(); // auto-repeat of a swallowed key
            }
        }
        if self.recording.is_some() {
            return self.record(ev);
        }
        if ev.key == Key::Code(self.escape) && ev.down && active {
            self.swallowed.push(self.escape);
            if let Some(p) = self.press.as_mut() {
                p.clean = false;
                p.done = true;
            }
            return Decision::swallow_and(Action::Cancel);
        }
        match self.hotkey {
            Hotkey::Modifier { key } => self.bare(key, ev, now),
            Hotkey::Combo { mods, code } => self.combo(mods, code, ev, now),
        }
    }

    fn bare(&mut self, hotkey: ModKey, ev: &KeyEvent, now: Instant) -> Decision {
        let ptt = self.mode == Mode::PushToTalk;
        if ev.key == Key::Mod(hotkey) {
            if ev.down {
                if self.press.is_some() {
                    return Decision::pass(); // repeat (Windows repeats modifiers)
                }
                let chord = !ev.mods.without(hotkey.mods()).is_empty();
                let start = ptt && !chord;
                self.press = Some(Press {
                    at: now,
                    clean: !chord,
                    started: start,
                    done: chord,
                });
                return if start {
                    Decision::act(Action::Start)
                } else {
                    Decision::pass()
                };
            }
            let Some(p) = self.press.take() else {
                return Decision::pass();
            };
            let held = now.saturating_duration_since(p.at);
            return match self.mode {
                Mode::Toggle if p.clean && held <= TAP_MAX => Decision::act(Action::Toggle),
                Mode::PushToTalk if p.started && !p.done => Decision::act(if held < HOLD_MIN {
                    Action::Cancel
                } else {
                    Action::Stop
                }),
                _ => Decision::pass(),
            };
        }
        // Another key while the hotkey is held: it was a chord, not a tap.
        if ev.down
            && !ev.repeat
            && let Some(p) = self.press.as_mut()
        {
            p.clean = false;
            if p.started && !p.done {
                p.done = true;
                return Decision::act(Action::Cancel);
            }
        }
        Decision::pass()
    }

    fn combo(&mut self, mods: Mods, code: u32, ev: &KeyEvent, now: Instant) -> Decision {
        if ev.key != Key::Code(code) || !ev.down {
            return Decision::pass();
        }
        if ev.mods != mods {
            return Decision::pass();
        }
        self.swallowed.push(code);
        self.combo_at = Some(now);
        Decision::swallow_and(match self.mode {
            Mode::Toggle => Action::Toggle,
            Mode::PushToTalk => Action::Start,
        })
    }

    /// Up of a swallowed key: ends a push-to-talk combo press.
    fn combo_up(&mut self, code: u32, now: Instant) -> Decision {
        let is_combo = matches!(self.hotkey, Hotkey::Combo { code: c, .. } if c == code);
        match self.combo_at.take() {
            Some(at) if is_combo && self.mode == Mode::PushToTalk => {
                let held = now.saturating_duration_since(at);
                Decision::swallow_and(if held < HOLD_MIN {
                    Action::Cancel
                } else {
                    Action::Stop
                })
            }
            other => {
                if !is_combo {
                    self.combo_at = other;
                }
                Decision::swallow()
            }
        }
    }

    fn record(&mut self, ev: &KeyEvent) -> Decision {
        let rec = self.recording.as_mut().expect("recording");
        match ev.key {
            Key::Mod(m) => {
                // Modifiers pass through so the OS never sees one stuck down.
                if ev.down {
                    if rec.modifier.is_none() && ev.mods.without(m.mods()).is_empty() {
                        rec.modifier = Some(m);
                    } else {
                        rec.dirty = true;
                    }
                    Decision::pass()
                } else if rec.modifier == Some(m) {
                    let clean = !rec.dirty;
                    rec.modifier = None;
                    rec.dirty = false;
                    if clean {
                        self.recording = None;
                        return Decision {
                            recorded: Some(Recorded::Hotkey(Hotkey::Modifier { key: m })),
                            ..Decision::pass()
                        };
                    }
                    Decision::pass()
                } else {
                    Decision::pass()
                }
            }
            Key::Code(code) => {
                if !ev.down {
                    return Decision::pass();
                }
                self.swallowed.push(code);
                rec.dirty = true;
                if ev.mods.is_empty() {
                    if code == self.escape {
                        self.recording = None;
                        return Decision {
                            swallow: true,
                            action: None,
                            recorded: Some(Recorded::Cancelled),
                        };
                    }
                    // A plain key would make typing impossible; ignore it.
                    return Decision::swallow();
                }
                self.recording = None;
                Decision {
                    swallow: true,
                    action: None,
                    recorded: Some(Recorded::Hotkey(Hotkey::Combo {
                        mods: ev.mods,
                        code,
                    })),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: u32 = 53;
    const A: u32 = 0;
    const SPACE: u32 = 49;

    struct T {
        m: Matcher,
        t0: Instant,
        now: Duration,
        active: bool,
    }

    fn bare(mode: Mode) -> T {
        T::new(Hotkey::default(), mode)
    }

    fn cmd_shift_space(mode: Mode) -> T {
        T::new(
            Hotkey::Combo {
                mods: Mods {
                    meta: true,
                    shift: true,
                    ..Mods::default()
                },
                code: SPACE,
            },
            mode,
        )
    }

    fn mods(keys: &[ModKey]) -> Mods {
        let mut m = Mods::default();
        for k in keys {
            let f = k.mods();
            m.ctrl |= f.ctrl;
            m.shift |= f.shift;
            m.alt |= f.alt;
            m.meta |= f.meta;
        }
        m
    }

    impl T {
        fn new(hk: Hotkey, mode: Mode) -> Self {
            Self {
                m: Matcher::new(hk, mode, ESC),
                t0: Instant::now(),
                now: Duration::ZERO,
                active: false,
            }
        }

        fn at(&mut self, ms: u64) -> &mut Self {
            self.now = Duration::from_millis(ms);
            self
        }

        fn ev(&mut self, key: Key, down: bool, held: &[ModKey]) -> Decision {
            let ev = KeyEvent {
                key,
                down,
                mods: mods(held),
                repeat: false,
            };
            self.m.handle(&ev, self.t0 + self.now, self.active)
        }

        fn ropt(&mut self, down: bool) -> Decision {
            let held: &[ModKey] = if down { &[ModKey::RightAlt] } else { &[] };
            self.ev(Key::Mod(ModKey::RightAlt), down, held)
        }
    }

    #[test]
    fn toggle_tap_of_bare_modifier() {
        let mut t = bare(Mode::Toggle);
        assert_eq!(t.at(0).ropt(true), Decision::pass());
        assert_eq!(t.at(120).ropt(false), Decision::act(Action::Toggle));
    }

    #[test]
    fn toggle_long_hold_is_not_a_tap() {
        let mut t = bare(Mode::Toggle);
        t.at(0).ropt(true);
        assert_eq!(t.at(700).ropt(false), Decision::pass());
    }

    #[test]
    fn option_letter_types_normally() {
        let mut t = bare(Mode::Toggle);
        t.at(0).ropt(true);
        let d = t.at(50).ev(Key::Code(A), true, &[ModKey::RightAlt]);
        assert_eq!(d, Decision::pass());
        t.at(80).ev(Key::Code(A), false, &[ModKey::RightAlt]);
        assert_eq!(t.at(100).ropt(false), Decision::pass());
    }

    #[test]
    fn chord_with_another_modifier_is_not_a_tap() {
        let mut t = bare(Mode::Toggle);
        t.at(0)
            .ev(Key::Mod(ModKey::LeftMeta), true, &[ModKey::LeftMeta]);
        t.at(10).ev(
            Key::Mod(ModKey::RightAlt),
            true,
            &[ModKey::LeftMeta, ModKey::RightAlt],
        );
        assert_eq!(
            t.at(50)
                .ev(Key::Mod(ModKey::RightAlt), false, &[ModKey::LeftMeta]),
            Decision::pass()
        );
        // And the other way round: hotkey first, then another modifier.
        t.at(1000).ropt(true);
        t.at(1010).ev(
            Key::Mod(ModKey::LeftShift),
            true,
            &[ModKey::RightAlt, ModKey::LeftShift],
        );
        assert_eq!(t.at(1050).ropt(false), Decision::pass());
    }

    #[test]
    fn repeated_modifier_downs_are_one_press() {
        let mut t = bare(Mode::Toggle);
        t.at(0).ropt(true);
        t.at(300).ropt(true);
        t.at(500).ropt(true);
        assert_eq!(t.at(550).ropt(false), Decision::act(Action::Toggle));
    }

    #[test]
    fn ptt_bare_start_and_stop() {
        let mut t = bare(Mode::PushToTalk);
        assert_eq!(t.at(0).ropt(true), Decision::act(Action::Start));
        t.active = true;
        assert_eq!(t.at(2000).ropt(false), Decision::act(Action::Stop));
    }

    #[test]
    fn ptt_short_press_cancels() {
        let mut t = bare(Mode::PushToTalk);
        t.at(0).ropt(true);
        assert_eq!(t.at(100).ropt(false), Decision::act(Action::Cancel));
    }

    #[test]
    fn ptt_other_key_cancels_once() {
        let mut t = bare(Mode::PushToTalk);
        t.at(0).ropt(true);
        let d = t.at(300).ev(Key::Code(A), true, &[ModKey::RightAlt]);
        assert_eq!(d, Decision::act(Action::Cancel));
        let d = t.at(400).ev(Key::Code(A), true, &[ModKey::RightAlt]);
        assert_eq!(d, Decision::pass());
        assert_eq!(t.at(900).ropt(false), Decision::pass());
    }

    #[test]
    fn ptt_chord_does_not_start() {
        let mut t = bare(Mode::PushToTalk);
        let d = t.at(0).ev(
            Key::Mod(ModKey::RightAlt),
            true,
            &[ModKey::LeftMeta, ModKey::RightAlt],
        );
        assert_eq!(d, Decision::pass());
        assert_eq!(
            t.at(900)
                .ev(Key::Mod(ModKey::RightAlt), false, &[ModKey::LeftMeta]),
            Decision::pass()
        );
    }

    #[test]
    fn escape_only_taken_while_active() {
        let mut t = bare(Mode::Toggle);
        assert_eq!(t.at(0).ev(Key::Code(ESC), true, &[]), Decision::pass());
        assert_eq!(t.at(10).ev(Key::Code(ESC), false, &[]), Decision::pass());
        t.active = true;
        assert_eq!(
            t.at(20).ev(Key::Code(ESC), true, &[]),
            Decision::swallow_and(Action::Cancel)
        );
        // The session ends before the key comes up; the up is still swallowed.
        t.active = false;
        assert_eq!(t.at(30).ev(Key::Code(ESC), false, &[]), Decision::swallow());
        assert_eq!(t.at(40).ev(Key::Code(ESC), true, &[]), Decision::pass());
    }

    #[test]
    fn escape_repeat_is_swallowed() {
        let mut t = bare(Mode::Toggle);
        t.active = true;
        t.at(0).ev(Key::Code(ESC), true, &[]);
        assert_eq!(t.at(500).ev(Key::Code(ESC), true, &[]), Decision::swallow());
        assert_eq!(
            t.at(600).ev(Key::Code(ESC), false, &[]),
            Decision::swallow()
        );
    }

    #[test]
    fn ptt_escape_while_holding_cancels_once() {
        let mut t = bare(Mode::PushToTalk);
        t.at(0).ropt(true);
        t.active = true;
        let d = t.at(500).ev(Key::Code(ESC), true, &[ModKey::RightAlt]);
        assert_eq!(d, Decision::swallow_and(Action::Cancel));
        t.at(550).ev(Key::Code(ESC), false, &[ModKey::RightAlt]);
        assert_eq!(t.at(900).ropt(false), Decision::pass());
    }

    #[test]
    fn toggle_combo_swallows_down_repeat_and_up() {
        let mut t = cmd_shift_space(Mode::Toggle);
        let held = [ModKey::LeftMeta, ModKey::LeftShift];
        assert_eq!(
            t.at(0).ev(Key::Code(SPACE), true, &held),
            Decision::swallow_and(Action::Toggle)
        );
        assert_eq!(
            t.at(400).ev(Key::Code(SPACE), true, &held),
            Decision::swallow()
        );
        // Modifiers released first; the space up must not leak.
        assert_eq!(
            t.at(500).ev(Key::Code(SPACE), false, &[]),
            Decision::swallow()
        );
    }

    #[test]
    fn combo_needs_exact_modifiers() {
        let mut t = cmd_shift_space(Mode::Toggle);
        let d = t.at(0).ev(Key::Code(SPACE), true, &[ModKey::LeftMeta]);
        assert_eq!(d, Decision::pass());
        let d = t.at(10).ev(
            Key::Code(SPACE),
            true,
            &[ModKey::LeftMeta, ModKey::LeftShift, ModKey::LeftAlt],
        );
        assert_eq!(d, Decision::pass());
        // Right-side modifiers count as well.
        let d = t.at(20).ev(
            Key::Code(SPACE),
            true,
            &[ModKey::RightMeta, ModKey::RightShift],
        );
        assert_eq!(d, Decision::swallow_and(Action::Toggle));
    }

    #[test]
    fn ptt_combo_hold_and_short_press() {
        let mut t = cmd_shift_space(Mode::PushToTalk);
        let held = [ModKey::LeftMeta, ModKey::LeftShift];
        assert_eq!(
            t.at(0).ev(Key::Code(SPACE), true, &held),
            Decision::swallow_and(Action::Start)
        );
        assert_eq!(
            t.at(1500).ev(Key::Code(SPACE), false, &held),
            Decision::swallow_and(Action::Stop)
        );
        t.at(2000).ev(Key::Code(SPACE), true, &held);
        assert_eq!(
            t.at(2100).ev(Key::Code(SPACE), false, &held),
            Decision::swallow_and(Action::Cancel)
        );
    }

    #[test]
    fn records_a_bare_modifier() {
        let mut t = bare(Mode::Toggle);
        t.m.start_recording();
        let d = t
            .at(0)
            .ev(Key::Mod(ModKey::RightMeta), true, &[ModKey::RightMeta]);
        assert_eq!(d, Decision::pass());
        let d = t.at(100).ev(Key::Mod(ModKey::RightMeta), false, &[]);
        assert_eq!(
            d.recorded,
            Some(Recorded::Hotkey(Hotkey::Modifier {
                key: ModKey::RightMeta
            }))
        );
        assert!(!t.m.is_recording());
        // Recording does not act on the current hotkey.
        assert_eq!(t.at(200).ropt(true), Decision::pass());
    }

    #[test]
    fn records_a_combo_and_swallows_it() {
        let mut t = bare(Mode::Toggle);
        t.m.start_recording();
        t.at(0)
            .ev(Key::Mod(ModKey::LeftCtrl), true, &[ModKey::LeftCtrl]);
        t.at(10).ev(
            Key::Mod(ModKey::LeftAlt),
            true,
            &[ModKey::LeftCtrl, ModKey::LeftAlt],
        );
        let d = t
            .at(20)
            .ev(Key::Code(SPACE), true, &[ModKey::LeftCtrl, ModKey::LeftAlt]);
        assert!(d.swallow);
        assert_eq!(
            d.recorded,
            Some(Recorded::Hotkey(Hotkey::Combo {
                mods: mods(&[ModKey::LeftCtrl, ModKey::LeftAlt]),
                code: SPACE
            }))
        );
        assert_eq!(
            t.at(30).ev(Key::Code(SPACE), false, &[]),
            Decision::swallow()
        );
    }

    #[test]
    fn recording_ignores_plain_keys_and_escape_cancels() {
        let mut t = bare(Mode::Toggle);
        t.m.start_recording();
        assert_eq!(t.at(0).ev(Key::Code(A), true, &[]), Decision::swallow());
        assert_eq!(t.at(10).ev(Key::Code(A), false, &[]), Decision::swallow());
        assert!(t.m.is_recording());
        let d = t.at(20).ev(Key::Code(ESC), true, &[]);
        assert_eq!(d.recorded, Some(Recorded::Cancelled));
        assert!(d.swallow);
        assert_eq!(t.at(30).ev(Key::Code(ESC), false, &[]), Decision::swallow());
    }

    #[test]
    fn recording_two_modifiers_then_release_records_nothing() {
        let mut t = bare(Mode::Toggle);
        t.m.start_recording();
        t.at(0)
            .ev(Key::Mod(ModKey::LeftCtrl), true, &[ModKey::LeftCtrl]);
        t.at(10).ev(
            Key::Mod(ModKey::LeftAlt),
            true,
            &[ModKey::LeftCtrl, ModKey::LeftAlt],
        );
        t.at(20)
            .ev(Key::Mod(ModKey::LeftAlt), false, &[ModKey::LeftCtrl]);
        let d = t.at(30).ev(Key::Mod(ModKey::LeftCtrl), false, &[]);
        assert_eq!(d.recorded, None);
        assert!(t.m.is_recording());
    }
}
