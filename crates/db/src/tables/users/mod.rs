//! `users` table. `entity` holds the SeaORM schema; `crud` holds every
//! operation (the only place a `users` `ActiveModel` is built). Both are
//! re-exported so `wf_db::tables::users` exposes `Model`, `Column`, … and
//! `upsert_from_auth` through a single import.

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
