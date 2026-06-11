//! `slack_connections` table. `entity` holds the SeaORM schema; `crud` holds
//! every operation (the only place a `slack_connections` `ActiveModel` is built).

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
