use crate::common::TempDatabase;
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
fn test_pg_unnest_expands_array(db: TempDatabase) {
    let conn = db.connect_postgres();
    let out = query_integer(&conn, "SELECT unnest(ARRAY[1, 2, 3]) ORDER BY 1");
    assert_eq!(out, [1, 2, 3]);
}

#[turso_macros::test]
fn test_pg_array_subquery(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer)").unwrap();
    conn.execute("INSERT INTO t VALUES (10), (20), (30)")
        .unwrap();

    let out = query_integer(&conn, "SELECT unnest(ARRAY[10, 20, 30]) ORDER BY 1");
    assert_eq!(out, [10, 20, 30]);
}
