//! One module per database table. Each sub-module owns its SeaORM entity
//! (the schema) *and* every CRUD operation for that table, so all reads/writes
//! — and the single typed `ActiveModel` construction site — are centralized in
//! one directory per table. Callers import one path per table, e.g.
//! `use wf_db::tables::github_pat_connections as gh;`, which yields both the
//! schema items (`gh::Model`, `gh::Column`, …) and the operations
//! (`gh::upsert_pat`, `gh::select_row`, …).

pub mod github_pat_connections;
pub mod jira_pat_connections;
pub mod users;
