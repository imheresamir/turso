use crate::common::TempDatabase;
use std::sync::Arc;
use turso_core::{Numeric, StepResult, Value};

fn query_integer(conn: &turso_pg::PgConnection, sql: &str) -> Vec<i64> {
    let mut rows = conn.query(sql).unwrap().unwrap();
    let mut result = Vec::new();
    loop {
        match rows.step().unwrap() {
            StepResult::Row => {
                let row = rows.row().unwrap();
                match row.get_value(0) {
                    Value::Numeric(Numeric::Integer(v)) => result.push(*v),
                    Value::Null => result.push(0),
                    other => panic!("expected integer, got {other:?}"),
                }
            }
            StepResult::Done => break,
            _ => {}
        }
    }
    result
}

#[turso_macros::test]
fn test_pg_txid_current_returns_xid(db: TempDatabase) {
    let conn = db.connect_postgres();
    // txid_current reports the current transaction id (non-zero once a
    // transaction has started).
    assert_eq!(query_integer(&conn, "SELECT txid_current()"), [1]);
}

#[turso_macros::test]
fn test_pg_relation_size_uses_injected_fn(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer PRIMARY KEY)")
        .unwrap();

    // The compat layer reports relation size through an embedder-supplied
    // closure; without one the functions fall back to 0.
    conn.set_relation_size_fn(Arc::new(|| 4096));
    assert_eq!(query_integer(&conn, "SELECT pg_table_size('t')"), [4096]);
    assert_eq!(query_integer(&conn, "SELECT pg_relation_size('t')"), [4096]);
    assert_eq!(
        query_integer(&conn, "SELECT pg_total_relation_size('t')"),
        [4096]
    );
}
