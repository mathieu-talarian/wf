//! Versioned DDL for the event backbone (A1 spec §7.1). Raw SQL migrations:
//! the SQL is the source of truth; hand-written entities in `wf-db` match it.

pub use sea_orm_migration::prelude::*;

mod m0001_create_events;
mod m0002_create_sync_state;
mod m0003_create_hub_tables;
mod m0004_add_hub_read_indexes;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_create_events::Migration),
            Box::new(m0002_create_sync_state::Migration),
            Box::new(m0003_create_hub_tables::Migration),
            Box::new(m0004_add_hub_read_indexes::Migration),
        ]
    }
}
