//! Rewriting a transcript as written text, by whichever service is chosen.

mod openai;

use std::time::Duration;

use crate::doubao::ime::ImeClient;

pub use openai::{DEFAULT_PROMPT, OpenAiOrganizer};

#[derive(Clone)]
pub enum Organizer {
    DoubaoIme(ImeClient),
    OpenAi(OpenAiOrganizer),
}

impl Organizer {
    /// `None` when the service fails, takes longer than `timeout` or
    /// answers with nothing; the caller then keeps the original text.
    pub async fn organize(&self, text: &str, timeout: Duration) -> Option<String> {
        match self {
            Organizer::DoubaoIme(ime) => ime.organize(text, timeout).await,
            Organizer::OpenAi(o) => o.organize(text, timeout).await,
        }
    }
}
