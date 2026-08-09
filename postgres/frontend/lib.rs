mod aliases;
mod catalog;
mod copy;
mod functions;
mod session;

pub use session::PgConnection as Connection;
pub use session::{
    open_database, open_database_with_io, open_database_with_storage, split_statements,
    PgConnection, PgQueryRunner,
};
pub use turso_core::{
    Database, DatabaseOpts, DatabaseStorage, Func, LimboError, Numeric, OpenFlags, PlatformIO,
    Result, StepResult,
};

/// The PostgreSQL schema dialect. Opening a database with this dialect (via
/// `open_database` / `open_database_with_io` / `open_database_with_storage`)
/// registers the `pg_catalog` virtual tables (`pg_class`, `pg_namespace`, …)
/// that the PostgreSQL frontend relies on for introspection. Exposed so
/// embedders can build a `turso_core::OpenOptions` with a custom storage
/// backend and still install the PostgreSQL catalog.
pub use catalog::PostgresDialect;

pub mod vtab {
    pub use turso_core::VirtualTable;
}
