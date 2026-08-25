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

// `EXPLAIN ANALYZE` shows the plan but does NOT execute the statement. An
// `EXPLAIN ANALYZE INSERT` must leave the table empty (no rows landed),
// unlike PostgreSQL where ANALYZE runs the statement.
#[turso_macros::test]
fn test_pg_explain_analyze_does_not_execute(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer PRIMARY KEY)")
        .unwrap();

    // This would insert (5) if ANALYZE actually ran the statement.
    conn.query("EXPLAIN ANALYZE INSERT INTO t VALUES (5)")
        .unwrap()
        .unwrap();

    // No row should have landed.
    let n = count_rows(&conn, "SELECT id FROM t");
    assert_eq!(n, 0, "EXPLAIN ANALYZE must not execute the statement");
}

// Plain `EXPLAIN` must also not execute the statement.
#[turso_macros::test]
fn test_pg_explain_does_not_execute(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer PRIMARY KEY)")
        .unwrap();

    conn.query("EXPLAIN INSERT INTO t VALUES (5)")
        .unwrap()
        .unwrap();

    let n = count_rows(&conn, "SELECT id FROM t");
    assert_eq!(n, 0, "EXPLAIN must not execute the statement");
}

// Unsupported EXPLAIN options are rejected with a clear error rather than
// silently ignored (PostgreSQL's VERBOSE/BUFFERS change the output format).
#[turso_macros::test]
fn test_pg_explain_option_rejected(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer)").unwrap();

    let err = conn
        .query("EXPLAIN (BUFFERS) SELECT id FROM t")
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("not supported"),
        "EXPLAIN option must be rejected: {err:?}"
    );
}
