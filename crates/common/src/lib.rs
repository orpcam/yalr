//! Gemeinsame Typen: AppState (DB-Pools, Caches, Routing) + Crypto.
//! Liegt in einer eigenen Crate, damit gateway und dashboard beide davon
//! abhaengen koennen ohne Zyklen.

pub mod state;
pub mod crypto;
pub mod auth;
pub mod metrics;

pub use state::{AppState, AppStateInner, KeySpend, MetricsSnapshot, VirtualKey};
