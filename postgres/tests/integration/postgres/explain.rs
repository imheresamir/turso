use crate::common::TempDatabase;
use turso_core::StepResult;

fn count_rows(conn: &turso_pg::PgConnection, sql: &str) -> usize {
    let mut rows = conn.query(sql).unwrap().unwrap();
    let mut n = 0;
    loop {
        match rows.step().unwrap() {
            StepResult::Row => n += 1,
            StepResult::Done => break,
            _ => {}
        }
    }
    n
}

#[turso_macros::test]
fn test_pg_explain_returns_plan(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer PRIMARY KEY)")
        .unwrap();

    // EXPLAIN must produce a plan (at least one output row) without error.
    let n = count_rows(&conn, "EXPLAIN SELECT id FROM t WHERE id = 1");
    assert!(n >= 1, "EXPLAIN must return a plan");
}

#[turso_macros::test]
fn test_pg_explain_result_is_readable(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer PRIMARY KEY)")
        .unwrap();

    // Each plan row must be a value we can read back.
    let mut rows = conn.query("EXPLAIN SELECT id FROM t").unwrap().unwrap();
    loop {
        match rows.step().unwrap() {
            StepResult::Row => {
                let _ = rows.row().unwrap().get_value(0);
            }
            StepResult::Done => break,
            _ => {}
        }
    }
}
