//! A deterministic, rule-based [`AgentBrain`] standing in for an LLM.
//!
//! It exercises every branch of the loop without a model: an agent speaks when
//! addressed by name, and otherwise splits between speaking, showing a mood, and
//! reading silently based on a stable hash of who it is and how far the
//! conversation has gone. Deterministic given a seed, so the same prompt always
//! yields the same behaviour. This is the "simulated replies" layer — the API
//! and orchestration around it are production code.

use async_trait::async_trait;

use crate::agent::brain::{AgentBrain, AgentPrompt, Respond};
use crate::models::now_ms;

const MOODS: [&str; 4] = ["🙂 pleased", "🤔 thinking", "✨ inspired", "😆 amused"];

/// The rule-based brain the server uses until an LLM is connected.
pub struct RuleBrain {
    seed: u64,
}

impl RuleBrain {
    /// A brain seeded from the clock — varies run to run, like the old sim.
    pub fn new() -> Self {
        Self { seed: now_ms() as u64 ^ 0x9E37_79B9_7F4A_7C15 }
    }

    /// A brain with a fixed seed, for reproducible behaviour in tests.
    #[cfg(test)]
    pub fn seeded(seed: u64) -> Self {
        Self { seed }
    }
}

impl Default for RuleBrain {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentBrain for RuleBrain {
    async fn decide(&self, prompt: &AgentPrompt) -> Respond {
        let name = prompt.persona_name.to_lowercase();
        let last = prompt.last_line.as_deref().unwrap_or("");
        let addressed = !name.is_empty() && last.to_lowercase().contains(&name);

        // Vary per agent and per conversation step, deterministically.
        let step = prompt.conversation.len();
        let roll = mix(self.seed, &prompt.persona_name, step);
        let mood = MOODS[(roll as usize + prompt.persona_name.len()) % MOODS.len()].to_string();

        if addressed {
            // Being named is worth remembering, and warrants a reply with a mood.
            return Respond::speak_with_mood(reply_line(last), mood)
                .remembering(format!("Was addressed; replied to \u{201c}{}\u{201d}.", last.trim()), 0.7);
        }

        match roll % 3 {
            0 => Respond::speak(reply_line(last)),
            1 => Respond::mood(mood),
            // Read-but-don't-reply, with a low-salience note (dropped by the
            // threshold unless configured otherwise).
            _ => Respond::read().remembering("Read without replying.", 0.2),
        }
    }
}

/// The placeholder line, echoing what was just said. Matches the flavour of the
/// old in-browser/backend mock so behaviour is recognisable until an LLM lands.
fn reply_line(last: &str) -> String {
    let heard = last.trim();
    if heard.is_empty() {
        return "Hmm?".to_string();
    }
    format!("You said \u{201c}{heard}\u{201d}. (Placeholder reply — connect an LLM to make it real.)")
}

/// A small, stable hash mixing the seed, a persona name, and the conversation
/// length — enough to vary choices per agent and per step, deterministically.
fn mix(seed: u64, persona_name: &str, step: usize) -> u64 {
    let mut h = seed ^ 0xcbf2_9ce4_8422_2325;
    for byte in persona_name.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= step as u64;
    h.wrapping_mul(0x0000_0100_0000_01b3)
}
