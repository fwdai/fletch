//! SQLite-backed storage. A thin generic CRUD layer over an allow-listed set
//! of tables, plus the connection lifecycle and typed settings accessors.
//!
//! Submodules: [`connection`] (open/migrate/backup), [`validate`] (identifier
//! guards), [`marshal`] (JSON⇄SQL), [`crud`] (the generic `db_*` API), and
//! [`settings`] (typed key-value accessors). Re-exported flat so existing
//! `crate::database::*` paths stay valid.

mod connection;
mod crud;
mod marshal;
mod settings;
mod validate;

#[cfg(test)]
mod tests;

pub use connection::*;
pub use crud::*;
pub use settings::*;
