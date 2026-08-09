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
