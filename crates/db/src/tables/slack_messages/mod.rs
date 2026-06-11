//! `slack_messages` table. `entity` holds the SeaORM schema; `crud` holds
//! every operation (the only place a `slack_messages` `ActiveModel` is built).

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
