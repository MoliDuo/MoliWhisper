//! A dictation session: microphone → ASR → text.

pub mod controller;
pub mod machine;

pub use controller::{Controller, Env, Update};
pub use machine::{Outcome, Phase, Timings};
