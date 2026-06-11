//! `reminders` table. `entity` holds the SeaORM schema; `crud` holds every
//! operation (the only place a `reminders` `ActiveModel` is built).

mod crud;
mod entity;

pub use crud::*;
pub use entity::*;
