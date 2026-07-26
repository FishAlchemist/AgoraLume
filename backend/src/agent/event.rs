//! Unified salience: one appraisal deciding how any incoming event is handled.
//!
//! Both Phase-1 conflict evaluation and mid-turn interruption go through
//! [`appraise`], so "how urgent is this?" is answered in exactly one place and
//! the two paths can never diverge.

/// Something entering a group: a fresh user message, or a change in the world.
#[derive(Clone, Debug)]
pub enum Event {
    /// The user sent a message. `message_id` is the stored line every agent
    /// marks as read this turn; the text itself already lives in the log.
    User { message_id: String },
    /// An environment change (rain, time passing, an emergency).
    Environment { description: String, urgent: bool },
    /// A manual retry of a turn suspended by a failed agent inference. Carries no
    /// data: the coordinator holds the pending trigger and resumes it, re-running
    /// only the agents that have not yet read the pending message.
    Retry,
}

impl Event {
    /// The text an agent should see for this event, if any — used when folding
    /// an event into the Context Stream at a pipeline boundary. A user message
    /// is already a conversation line, so it enters the transcript on its own.
    pub fn as_context(&self) -> Option<&str> {
        match self {
            Event::Environment { description, .. } => Some(description.as_str()),
            Event::User { .. } | Event::Retry => None,
        }
    }
}

/// How the system should treat an event that arrives while a turn is running.
/// One entry point for both conflict evaluation and interruption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Salience {
    /// Urgent or contradictory (the user corrects/retracts; an emergency):
    /// preempt now, discarding in-flight work, so agents never answer stale
    /// context.
    Hard,
    /// Ordinary ambient change: fold in at the next pipeline boundary without
    /// cutting the current agent.
    Soft,
    /// Not significant enough to act on.
    Ignore,
}

/// Classifies an event arriving mid-turn. (A user message arriving while the
/// group is idle just starts a turn; that path doesn't consult this.)
pub fn appraise(event: &Event) -> Salience {
    match event {
        // A new user message during a turn is a correction: preempt.
        Event::User { .. } => Salience::Hard,
        Event::Environment { urgent: true, .. } => Salience::Hard,
        // A non-urgent change carrying nothing to say isn't worth interrupting.
        Event::Environment { description, .. } if description.trim().is_empty() => Salience::Ignore,
        Event::Environment { .. } => Salience::Soft,
        // A retry that lands mid-turn is stale — a turn is already running, so
        // there is nothing suspended to resume. Drop it; the idle coordinator
        // handles a genuine retry directly.
        Event::Retry => Salience::Ignore,
    }
}
