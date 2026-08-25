//! Per-connection PostgreSQL-compat state that cannot live on the core
//! `Connection` type.
//!
//! The relation-size hook is an embedder-supplied closure over the backing
//! store, a concern of the PostgreSQL wire frontend rather than the core
//! engine. It is keyed by the address of the wrapped `Arc<Connection>` so the
//! frontend can reach it from a borrowed `&Connection` (the signature the core
//! dialect trait hands us); threading a frontend handle through that trait
//! would put PostgreSQL-compat surface area into core. `txid_current()`
//! minting lives on the core connection itself (`Connection::next_pg_txid`,
//! behind the `pg_compat` feature).
//!
//! Lifetime: [`PgConnectionInner`] owns the only strong path from a
//! `PgConnection` to its core connection and removes this entry in its
//! `Drop`, so an entry never outlives its connection. Two caveats are
//! accepted here: (a) if clones of the `Arc<Connection>` outlive the inner,
//! their scalar lookups lazily re-create an *empty* state, so a registered
//! relation-size hook silently stops applying to them; (b) addresses are
//! recycled by the allocator, but a stale address can only be reached while
//! no entry exists for it, and any new entry is created fresh — a recycled
//! address starts with default state, never inherits the old hook.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use turso_core::Connection;

/// Frontend-only state for a single core connection.
pub struct CompatState {
    /// Optional embedder hook reporting the backing store's on-disk byte size.
    relation_size_fn: Mutex<Option<Arc<dyn Fn() -> i64 + Send + Sync>>>,
}

impl CompatState {
    fn new() -> Self {
        Self {
            relation_size_fn: Mutex::new(None),
        }
    }

    /// Report the backing store's on-disk byte size, or 0 when no hook is set.
    pub fn relation_size(&self) -> i64 {
        self.relation_size_fn
            .lock()
            .unwrap()
            .as_ref()
            .map(|f| f())
            .unwrap_or(0)
    }

    /// Register the relation-size hook.
    pub fn set_relation_size_fn(&self, f: Arc<dyn Fn() -> i64 + Send + Sync>) {
        *self.relation_size_fn.lock().unwrap() = Some(f);
    }
}

// Address of the `Connection` behind the `Arc` is the key. The `Arc<Connection>`
// derefs to `Connection` at that same address, so a borrowed `&Connection`
// resolves to the right entry. Entries are removed in
// `PgConnectionInner::Drop`; no other code may retain a key past that point.
type Key = usize;

static STATES: std::sync::OnceLock<Mutex<HashMap<Key, Arc<CompatState>>>> =
    std::sync::OnceLock::new();

fn states() -> &'static Mutex<HashMap<Key, Arc<CompatState>>> {
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(conn: &Connection) -> Key {
    (conn as *const Connection) as Key
}

/// Fetch (inserting if absent) the frontend state for `conn`.
pub(crate) fn compat_state(conn: &Connection) -> Arc<CompatState> {
    let k = key(conn);
    let mut guard = states().lock().unwrap();
    guard
        .entry(k)
        .or_insert_with(|| Arc::new(CompatState::new()))
        .clone()
}

/// Remove the frontend state for the connection, freeing the map entry. Called
/// from `PgConnectionInner`'s `Drop` so the side-table does not leak.
pub(crate) fn drop_compat_state(conn: &Connection) {
    states().lock().unwrap().remove(&key(conn));
}
