//! Event-backbone sync engine (A1 spec): cursors, normalizers, and the tick
//! routine. A library so both `wf-api` (in-process scheduler) and the future
//! `wf-worker` (roadmap Approach 2) share the exact same logic.

pub mod cursor;
pub mod normalize;
pub mod slack;
pub mod tick;

pub use tick::{run_tick, TickError, TickOptions, TickSummary};
