//! The multi-agent conversation engine.
//!
//! Production orchestration (the Global/Local loops, the two streams,
//! interrupts) around a single swappable seam, [`brain::AgentBrain`]. The
//! bundled [`mock::RuleBrain`] lets the server run without a model; replacing it
//! with an LLM-backed brain — and nothing else — turns on real replies. See
//! `agent_loop_arch.md` for the design.

pub mod brain;
pub mod event;
pub mod llm;
pub mod mock;
pub mod turn;
