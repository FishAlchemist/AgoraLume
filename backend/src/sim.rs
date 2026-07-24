//! The simulated "agents' turn": placeholder replies with realistic timing.
//!
//! This is the one piece a real LLM will replace — the API around it stays put.
//! It mirrors the frontend's in-browser mock: every AI member reads (processes)
//! the user's message, but only one — chosen at random — actually replies. The
//! rest read without replying, so the composer still unlocks once everyone is
//! done.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

use crate::models::Message;
use crate::state::AppState;

const MOODS: [&str; 4] = ["🙂 pleased", "🤔 thinking", "✨ inspired", "😆 amused"];

/// Simulates the reply turn for a freshly received user message. Returns
/// immediately; the mood, reply, and read receipts arrive on the group's stream
/// over the next second or so.
pub fn schedule_turn(state: Arc<AppState>, group_id: String, message_id: String, user_text: String) {
    let Some((_self_id, readers)) = state.workspace().turn_members(&group_id) else {
        return;
    };
    if readers.is_empty() {
        return;
    }

    let replier = readers[pseudo_random(readers.len())].clone();
    let mood = MOODS[pseudo_random(MOODS.len())].to_string();
    let reply = placeholder_reply(&user_text);

    for (i, reader) in readers.into_iter().enumerate() {
        let state = state.clone();
        let group_id = group_id.clone();
        let message_id = message_id.clone();

        if reader == replier {
            let mood = mood.clone();
            let reply = reply.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(500)).await;
                state.emit(&group_id, Message::mood(&group_id, &reader, mood, None));
                sleep(Duration::from_millis(600)).await;
                state.emit(&group_id, Message::conversation(&group_id, &reader, reply, None));
                state.mark_read(&group_id, &message_id, &reader);
            });
        } else {
            // Read-but-don't-reply: acknowledge processing without a message.
            tokio::spawn(async move {
                sleep(Duration::from_millis(400 + i as u64 * 160)).await;
                state.mark_read(&group_id, &message_id, &reader);
            });
        }
    }
}

/// A placeholder reply echoing the user's text, standing in until an LLM is
/// connected. Matches the frontend in-browser mock's copy.
fn placeholder_reply(user_text: &str) -> String {
    let text = user_text.trim();
    if text.is_empty() {
        return "Hmm?".to_string();
    }
    format!("You said \u{201c}{text}\u{201d}. (Placeholder reply — connect an LLM to make it real.)")
}

/// A cheap, dependency-free way to pick a member/mood for the placeholder turn:
/// it only needs to vary turn to turn, not be cryptographically random.
fn pseudo_random(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos ^ COUNTER.fetch_add(0x9e37_79b9, Ordering::Relaxed);
    (mixed % len as u64) as usize
}
