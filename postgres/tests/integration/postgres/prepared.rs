use crate::common::TempDatabase;
use turso_core::{StepResult, Value};

fn query_text(conn: &turso_pg::PgConnection, sql: &str) -> Vec<String> {
    let mut rows = conn.query(sql).unwrap().unwrap();
    let mut result = Vec::new();
    loop {
        match rows.step().unwrap() {
            StepResult::Row => {
                let row = rows.row().unwrap();
                match row.get_value(0) {
                    Value::Text(v) => result.push(v.value.to_string()),
                    Value::Null => result.push("NULL".to_string()),
                    other => panic!("expected text, got {other:?}"),
                }
            }
            StepResult::Done => break,
            _ => {}
        }
    }
    result
}

#[turso_macros::test]
fn test_pg_prepare_execute_deallocate(db: TempDatabase) {
    let conn = db.connect_postgres();

    conn.execute("CREATE TABLE t (id integer PRIMARY KEY, name text)")
        .unwrap();
    conn.execute("INSERT INTO t VALUES (1, 'alice'), (2, 'bob')")
        .unwrap();

    conn.execute("PREPARE q AS SELECT name FROM t WHERE id = $1")
        .unwrap();
    // Bind the parameter through EXECUTE.
    let out = query_text(&conn, "EXECUTE q(1)");
    assert_eq!(out, ["alice"]);

    let out = query_text(&conn, "EXECUTE q(2)");
    assert_eq!(out, ["bob"]);

    conn.execute("DEALLOCATE q").unwrap();
    // After DEALLOCATE the statement no longer exists.
    let err = conn.execute("EXECUTE q(1)").unwrap_err();
    assert!(
        format!("{err:?}").contains("does not exist"),
        "EXECUTE after DEALLOCATE must fail: {err:?}"
    );
}

#[turso_macros::test]
fn test_pg_deallocate_all(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("PREPARE a AS SELECT 1").unwrap();
    conn.execute("PREPARE b AS SELECT 2").unwrap();
    conn.execute("DEALLOCATE ALL").unwrap();
    let err = conn.execute("EXECUTE a").unwrap_err();
    assert!(format!("{err:?}").contains("does not exist"));
}

// Non-literal EXECUTE args are rejected with a clear error — matching
// PostgreSQL, which only allows literal EXECUTE parameters.
#[turso_macros::test]
fn test_pg_execute_nonliteral_param_rejected(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("PREPARE q AS SELECT $1").unwrap();
    let err = conn.execute("EXECUTE q(1 + 1)").unwrap_err();
    assert!(
        format!("{err:?}").contains("literal constants"),
        "non-literal EXECUTE arg must error, not bind NULL: {err:?}"
    );
}

// `EXECUTE` with mismatched arity binds the positional params it is given and
// leaves the surplus unbound (NULL) rather than erroring, so a too-few-args
// EXECUTE still runs with the missing param resolving to NULL. This documents
// the current behavior; PostgreSQL would reject the mismatch.
#[turso_macros::test]
fn test_pg_execute_arity_mismatch_binds_null(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("PREPARE q AS SELECT $1, $2").unwrap();

    // Too few args: the statement runs and the missing $2 resolves to NULL.
    let mut rows = conn.query("EXECUTE q(1)").unwrap().unwrap();
    loop {
        match rows.step().unwrap() {
            StepResult::Row => {
                let row = rows.row().unwrap();
                assert_eq!(
                    *row.get_value(0),
                    Value::Numeric(turso_core::Numeric::Integer(1))
                );
                assert_eq!(*row.get_value(1), Value::Null);
            }
            StepResult::Done => break,
            _ => {}
        }
    }
}
