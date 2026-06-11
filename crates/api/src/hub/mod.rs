//! Hub aggregation (`hub` tag): the board (Jira tickets × GitHub code ×
//! Slack/notes/reminder badges + orphan tray), the Needs-you inbox, the
//! actions-strip runs, and manual ticket↔code links.

pub mod board;
pub mod cache;
pub mod inbox;
pub mod routes;
pub mod runs;
pub mod types;
