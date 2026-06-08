//! `jira_pat_connections` table. `entity` holds the SeaORM schema; `crud` holds
//! every operation (the only place a `jira_pat_connections` `ActiveModel` is
//! built — insert/upsert type errors are confined here). Both are re-exported
//! so `wf_db::tables::jira_pat_connections` exposes `Model`, `Column`, the CRUD
//! functions, and `UpsertJiraInput` through a single import.

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
