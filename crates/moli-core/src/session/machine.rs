//! Dictation session state machine. Pure: `step` takes an event and returns
//! the effects to run; the controller does all IO.
//!
//! ```text
//! Idle ─Start→ Connecting ─Connected→ Recording ─Stop→ Finalizing ─finish/timeout→ Delivering ─→ Idle
//!                  │ Stop: remembered, finalize right after connecting
//! ```
//!
//! The microphone starts together with the connection; audio captured while
//! connecting is buffered and sent once connected, so no words are lost.
//! Every session has a new id and events from older sessions are ignored.

use std::time::Duration;

pub type Sid = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    /// Connection attempts per session (the first one included).
    pub max_attempts: u32,
    /// Budget for one attempt (DNS + TCP + TLS + WebSocket).
    pub connect_attempt: Duration,
    /// Give up connecting after this long, however many attempts are left.
    pub connect_deadline: Duration,
    /// How long to wait for `finish` after sending ours.
    pub finalize: Duration,
    /// Recording stops by itself after this long.
    pub max_duration: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            connect_attempt: Duration::from_millis(2500),
            connect_deadline: Duration::from_secs(5),
            finalize: Duration::from_secs(3),
            max_duration: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Connecting,
    Recording,
    Finalizing,
    Delivering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    ConnectDeadline,
    Finalize,
    MaxDuration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Start,
    Stop,
    Cancel,
    Connected {
        sid: Sid,
    },
    ConnectFailed {
        sid: Sid,
        reason: String,
    },
    /// The full transcript so far.
    Text {
        sid: Sid,
        text: String,
    },
    ServerFinished {
        sid: Sid,
    },
    SessionRejected {
        sid: Sid,
    },
    ConnectionLost {
        sid: Sid,
        reason: String,
    },
    AudioFailed {
        sid: Sid,
        reason: String,
    },
    Timeout {
        sid: Sid,
        timer: Timer,
    },
    Delivered {
        sid: Sid,
        result: Result<(), String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open the microphone and buffer its audio for the connection.
    StartAudio {
        sid: Sid,
    },
    /// Close the microphone; audio already captured still goes out.
    StopAudio,
    Connect {
        sid: Sid,
        attempt: u32,
    },
    /// Send the finish frame once all captured audio has been sent.
    Finish {
        sid: Sid,
    },
    /// Drop the connection and the microphone right away.
    Disconnect,
    Arm {
        sid: Sid,
        timer: Timer,
        after: Duration,
    },
    /// Live transcript for the overlay.
    Text(String),
    Deliver {
        sid: Sid,
        text: String,
    },
    /// Flag the stored login as refused by the server (never delete it).
    MarkRejected,
    Outcome(Outcome),
}

/// How a session ended, for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Text was delivered. `partial` when the connection broke off early.
    Done {
        partial: bool,
    },
    /// Nothing was recognized.
    Empty,
    Cancelled,
    /// No usable login; nothing was started.
    NeedLogin,
    SessionRejected,
    Network(String),
    Microphone(String),
    DeliveryFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Connecting {
        sid: Sid,
        attempt: u32,
        stop_pending: bool,
    },
    Recording {
        sid: Sid,
    },
    Finalizing {
        sid: Sid,
    },
    Delivering {
        sid: Sid,
        partial: bool,
    },
}

pub struct Machine {
    state: State,
    last_sid: Sid,
    text: String,
    timings: Timings,
}

impl Machine {
    pub fn new(timings: Timings) -> Self {
        Self {
            state: State::Idle,
            last_sid: 0,
            text: String::new(),
            timings,
        }
    }

    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    pub fn phase(&self) -> Phase {
        match self.state {
            State::Idle => Phase::Idle,
            State::Connecting { .. } => Phase::Connecting,
            State::Recording { .. } => Phase::Recording,
            State::Finalizing { .. } => Phase::Finalizing,
            State::Delivering { .. } => Phase::Delivering,
        }
    }

    /// The live session, if any.
    pub fn sid(&self) -> Option<Sid> {
        match self.state {
            State::Idle => None,
            State::Connecting { sid, .. }
            | State::Recording { sid }
            | State::Finalizing { sid }
            | State::Delivering { sid, .. } => Some(sid),
        }
    }

    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        use Effect as E;
        if let Some(sid) = event_sid(&event)
            && self.sid() != Some(sid)
        {
            return vec![]; // stale
        }
        let t = self.timings;
        match (&mut self.state, event) {
            (State::Idle, Event::Start) => {
                self.last_sid += 1;
                let sid = self.last_sid;
                self.text.clear();
                self.state = State::Connecting {
                    sid,
                    attempt: 1,
                    stop_pending: false,
                };
                vec![
                    E::StartAudio { sid },
                    E::Connect { sid, attempt: 1 },
                    E::Arm {
                        sid,
                        timer: Timer::ConnectDeadline,
                        after: t.connect_deadline,
                    },
                    E::Arm {
                        sid,
                        timer: Timer::MaxDuration,
                        after: t.max_duration,
                    },
                ]
            }
            (_, Event::Start) => vec![],

            // Connecting
            (
                State::Connecting { stop_pending, .. },
                Event::Stop
                | Event::Timeout {
                    timer: Timer::MaxDuration,
                    ..
                },
            ) => {
                if *stop_pending {
                    return vec![];
                }
                *stop_pending = true;
                vec![E::StopAudio]
            }
            (
                &mut State::Connecting {
                    sid, stop_pending, ..
                },
                Event::Connected { .. },
            ) => {
                if stop_pending {
                    self.finalize(sid)
                } else {
                    self.state = State::Recording { sid };
                    vec![]
                }
            }
            (State::Connecting { sid, attempt, .. }, Event::ConnectFailed { reason, .. }) => {
                if *attempt < t.max_attempts {
                    *attempt += 1;
                    tracing::info!(attempt = *attempt, "connect failed ({reason}); retrying");
                    vec![E::Connect {
                        sid: *sid,
                        attempt: *attempt,
                    }]
                } else {
                    self.fail(Outcome::Network(reason))
                }
            }
            (
                State::Connecting { .. },
                Event::Timeout {
                    timer: Timer::ConnectDeadline,
                    ..
                },
            ) => self.fail(Outcome::Network("connect timed out".into())),

            // Recording
            (
                &mut State::Recording { sid },
                Event::Stop
                | Event::Timeout {
                    timer: Timer::MaxDuration,
                    ..
                },
            ) => {
                let mut effects = vec![E::StopAudio];
                effects.extend(self.finalize(sid));
                effects
            }
            (State::Recording { .. }, Event::ServerFinished { .. }) => self.deliver(true),
            (State::Recording { .. }, Event::ConnectionLost { reason, .. }) => {
                if self.text.trim().is_empty() {
                    self.fail(Outcome::Network(reason))
                } else {
                    self.deliver(true)
                }
            }

            // Finalizing
            (State::Finalizing { .. }, Event::ServerFinished { .. }) => self.deliver(false),
            (
                State::Finalizing { .. },
                Event::Timeout {
                    timer: Timer::Finalize,
                    ..
                },
            ) => {
                tracing::warn!("no finish from the server in time; using the last result");
                self.deliver(false)
            }
            (State::Finalizing { .. }, Event::ConnectionLost { reason, .. }) => {
                tracing::warn!(
                    "connection lost while finalizing ({reason}); using the last result"
                );
                self.deliver(false)
            }

            // Any live session
            (State::Recording { .. } | State::Finalizing { .. }, Event::Text { text, .. }) => {
                self.text = text.clone();
                vec![E::Text(text)]
            }
            (
                State::Connecting { .. } | State::Recording { .. } | State::Finalizing { .. },
                Event::SessionRejected { .. },
            ) => {
                let mut effects = self.fail(Outcome::SessionRejected);
                effects.insert(1, E::MarkRejected);
                effects
            }
            (
                State::Connecting { .. } | State::Recording { .. },
                Event::AudioFailed { reason, .. },
            ) => self.fail(Outcome::Microphone(reason)),
            (
                State::Connecting { .. } | State::Recording { .. } | State::Finalizing { .. },
                Event::Cancel,
            ) => self.fail(Outcome::Cancelled),

            // Delivering
            (&mut State::Delivering { partial, .. }, Event::Delivered { result, .. }) => {
                self.state = State::Idle;
                vec![E::Outcome(match result {
                    Ok(()) => Outcome::Done { partial },
                    Err(e) => Outcome::DeliveryFailed(e),
                })]
            }

            _ => vec![],
        }
    }

    fn finalize(&mut self, sid: Sid) -> Vec<Effect> {
        self.state = State::Finalizing { sid };
        vec![
            Effect::Finish { sid },
            Effect::Arm {
                sid,
                timer: Timer::Finalize,
                after: self.timings.finalize,
            },
        ]
    }

    fn deliver(&mut self, partial: bool) -> Vec<Effect> {
        let sid = self.sid().expect("deliver from a live session");
        let text = self.text.trim().to_string();
        if text.is_empty() {
            self.state = State::Idle;
            return vec![Effect::Disconnect, Effect::Outcome(Outcome::Empty)];
        }
        self.state = State::Delivering { sid, partial };
        vec![Effect::Disconnect, Effect::Deliver { sid, text }]
    }

    fn fail(&mut self, outcome: Outcome) -> Vec<Effect> {
        self.state = State::Idle;
        vec![Effect::Disconnect, Effect::Outcome(outcome)]
    }
}

fn event_sid(event: &Event) -> Option<Sid> {
    match event {
        Event::Start | Event::Stop | Event::Cancel => None,
        Event::Connected { sid }
        | Event::ConnectFailed { sid, .. }
        | Event::Text { sid, .. }
        | Event::ServerFinished { sid }
        | Event::SessionRejected { sid }
        | Event::ConnectionLost { sid, .. }
        | Event::AudioFailed { sid, .. }
        | Event::Timeout { sid, .. }
        | Event::Delivered { sid, .. } => Some(*sid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Effect as E;

    fn machine() -> Machine {
        Machine::new(Timings::default())
    }

    /// Starts a session and connects it; returns its sid.
    fn recording(m: &mut Machine) -> Sid {
        m.step(Event::Start);
        let sid = m.sid().unwrap();
        m.step(Event::Connected { sid });
        assert_eq!(m.phase(), Phase::Recording);
        sid
    }

    fn text(sid: Sid, t: &str) -> Event {
        Event::Text {
            sid,
            text: t.into(),
        }
    }

    fn outcome(effects: &[Effect]) -> Option<&Outcome> {
        effects.iter().find_map(|e| match e {
            E::Outcome(o) => Some(o),
            _ => None,
        })
    }

    #[test]
    fn happy_path() {
        let mut m = machine();
        let t = Timings::default();
        let fx = m.step(Event::Start);
        let sid = m.sid().unwrap();
        assert_eq!(
            fx,
            vec![
                E::StartAudio { sid },
                E::Connect { sid, attempt: 1 },
                E::Arm {
                    sid,
                    timer: Timer::ConnectDeadline,
                    after: t.connect_deadline
                },
                E::Arm {
                    sid,
                    timer: Timer::MaxDuration,
                    after: t.max_duration
                },
            ]
        );
        assert_eq!(m.step(Event::Connected { sid }), vec![]);
        assert_eq!(m.step(text(sid, "你好")), vec![E::Text("你好".into())]);
        assert_eq!(
            m.step(Event::Stop),
            vec![
                E::StopAudio,
                E::Finish { sid },
                E::Arm {
                    sid,
                    timer: Timer::Finalize,
                    after: t.finalize
                }
            ]
        );
        m.step(text(sid, "你好世界 "));
        assert_eq!(
            m.step(Event::ServerFinished { sid }),
            vec![
                E::Disconnect,
                E::Deliver {
                    sid,
                    text: "你好世界".into()
                }
            ]
        );
        assert_eq!(m.phase(), Phase::Delivering);
        assert_eq!(m.step(Event::Start), vec![], "busy while delivering");
        let fx = m.step(Event::Delivered {
            sid,
            result: Ok(()),
        });
        assert_eq!(outcome(&fx), Some(&Outcome::Done { partial: false }));
        assert_eq!(m.phase(), Phase::Idle);
    }

    #[test]
    fn stop_while_connecting_finalizes_after_connect() {
        let mut m = machine();
        m.step(Event::Start);
        let sid = m.sid().unwrap();
        assert_eq!(m.step(Event::Stop), vec![E::StopAudio]);
        assert_eq!(m.step(Event::Stop), vec![], "second stop is a no-op");
        assert_eq!(m.phase(), Phase::Connecting);
        let fx = m.step(Event::Connected { sid });
        assert_eq!(fx[0], E::Finish { sid });
        assert_eq!(m.phase(), Phase::Finalizing);
    }

    #[test]
    fn retries_then_gives_up() {
        let mut m = machine();
        m.step(Event::Start);
        let sid = m.sid().unwrap();
        let fail = || Event::ConnectFailed {
            sid,
            reason: "boom".into(),
        };
        assert_eq!(m.step(fail()), vec![E::Connect { sid, attempt: 2 }]);
        assert_eq!(m.step(fail()), vec![E::Connect { sid, attempt: 3 }]);
        let fx = m.step(fail());
        assert_eq!(fx[0], E::Disconnect);
        assert_eq!(outcome(&fx), Some(&Outcome::Network("boom".into())));
        assert_eq!(m.phase(), Phase::Idle);
    }

    #[test]
    fn connect_deadline() {
        let mut m = machine();
        m.step(Event::Start);
        let sid = m.sid().unwrap();
        let fx = m.step(Event::Timeout {
            sid,
            timer: Timer::ConnectDeadline,
        });
        assert!(matches!(outcome(&fx), Some(Outcome::Network(_))));
        // The deadline means nothing once connected.
        let sid = recording(&mut m);
        let fx = m.step(Event::Timeout {
            sid,
            timer: Timer::ConnectDeadline,
        });
        assert_eq!(fx, vec![]);
        assert_eq!(m.phase(), Phase::Recording);
    }

    #[test]
    fn rejection_marks_and_never_retries() {
        for connected in [false, true] {
            let mut m = machine();
            m.step(Event::Start);
            let sid = m.sid().unwrap();
            if connected {
                m.step(Event::Connected { sid });
            }
            let fx = m.step(Event::SessionRejected { sid });
            assert_eq!(
                fx,
                vec![
                    E::Disconnect,
                    E::MarkRejected,
                    E::Outcome(Outcome::SessionRejected)
                ]
            );
            assert_eq!(m.phase(), Phase::Idle);
        }
    }

    #[test]
    fn finalize_timeout_and_lost_connection_use_last_result() {
        for event in [
            |sid| Event::Timeout {
                sid,
                timer: Timer::Finalize,
            },
            |sid| Event::ConnectionLost {
                sid,
                reason: "reset".into(),
            },
        ] {
            let mut m = machine();
            let sid = recording(&mut m);
            m.step(text(sid, "最后"));
            m.step(Event::Stop);
            let fx = m.step(event(sid));
            assert_eq!(
                fx,
                vec![
                    E::Disconnect,
                    E::Deliver {
                        sid,
                        text: "最后".into()
                    }
                ]
            );
        }
    }

    #[test]
    fn empty_text_delivers_nothing() {
        let mut m = machine();
        let sid = recording(&mut m);
        m.step(text(sid, "  "));
        m.step(Event::Stop);
        let fx = m.step(Event::ServerFinished { sid });
        assert_eq!(fx, vec![E::Disconnect, E::Outcome(Outcome::Empty)]);
        assert_eq!(m.phase(), Phase::Idle);
    }

    #[test]
    fn connection_lost_while_recording() {
        let mut m = machine();
        let sid = recording(&mut m);
        let lost = Event::ConnectionLost {
            sid,
            reason: "reset".into(),
        };
        let fx = m.step(lost.clone());
        assert_eq!(outcome(&fx), Some(&Outcome::Network("reset".into())));

        let sid = recording(&mut m);
        m.step(text(sid, "说到一半"));
        let fx = m.step(Event::ConnectionLost {
            sid,
            reason: "reset".into(),
        });
        assert!(fx.contains(&E::Deliver {
            sid,
            text: "说到一半".into()
        }));
        let fx = m.step(Event::Delivered {
            sid,
            result: Ok(()),
        });
        assert_eq!(outcome(&fx), Some(&Outcome::Done { partial: true }));
    }

    #[test]
    fn cancel_and_audio_failure() {
        let mut m = machine();
        m.step(Event::Start);
        assert_eq!(
            m.step(Event::Cancel),
            vec![E::Disconnect, E::Outcome(Outcome::Cancelled)]
        );
        let sid = recording(&mut m);
        let fx = m.step(Event::AudioFailed {
            sid,
            reason: "unplugged".into(),
        });
        assert_eq!(outcome(&fx), Some(&Outcome::Microphone("unplugged".into())));
        assert_eq!(m.step(Event::Cancel), vec![], "nothing to cancel");
    }

    #[test]
    fn max_duration_stops() {
        let mut m = machine();
        let sid = recording(&mut m);
        let fx = m.step(Event::Timeout {
            sid,
            timer: Timer::MaxDuration,
        });
        assert_eq!(fx[..2], [E::StopAudio, E::Finish { sid }]);
    }

    #[test]
    fn stale_events_are_ignored() {
        let mut m = machine();
        let old = recording(&mut m);
        m.step(Event::Cancel);
        let new = recording(&mut m);
        assert_ne!(old, new);
        for e in [
            text(old, "旧"),
            Event::ServerFinished { sid: old },
            Event::SessionRejected { sid: old },
            Event::Delivered {
                sid: old,
                result: Ok(()),
            },
        ] {
            assert_eq!(m.step(e), vec![]);
        }
        assert_eq!(m.phase(), Phase::Recording);
    }
}
