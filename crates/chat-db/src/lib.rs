//! Concrete [`Store`](chat_store::Store) backends for rust-chat.
//!
//! Both engines live in this crate behind the `postgres` / `sqlite` features so
//! that every new `Store` method is one trait addition plus its two impls,
//! side by side, instead of a change across two sibling crates.

#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;
