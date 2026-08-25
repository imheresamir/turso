use crate::common::TempDatabase;

// Regression test for the TVF-lift bug: a table-valued function in the target
// list alongside a real `FROM` (here `unnest(ARRAY[id]) FROM t`) must NOT be
// clobbered into a FROM-less `TableCall` scan that drops `FROM t`. The engine
// does not support per-row `unnest(arr) FROM t` expansion, so the correct
// outcome is a clean error — not a corrupted rewrite that leaves `id` out of
// scope (the previous `no such column: id` corruption).
#[turso_macros::test]
fn test_pg_unnest_over_table_not_corrupted(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer)").unwrap();
    conn.execute("INSERT INTO t VALUES (10), (20), (30)")
        .unwrap();

    let result = conn.query("SELECT unnest(ARRAY[id]) FROM t");
    assert!(
        result.is_err(),
        "query with TVF in target list + real FROM must error cleanly, not corrupt"
    );
}

// `array(<subquery>)` with ORDER BY or LIMIT is rejected: element order and
// cardinality are part of the result, and silently dropping them would
// mis-evaluate (e.g. returning all rows where LIMIT 2 was asked).
#[turso_macros::test]
fn test_pg_array_subquery_order_by_limit_rejected(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE t (id integer)").unwrap();
    conn.execute("INSERT INTO t VALUES (3), (1), (2)").unwrap();

    for sql in [
        "SELECT array(SELECT id FROM t ORDER BY id DESC LIMIT 2)",
        "SELECT array(SELECT id FROM t ORDER BY id)",
        "SELECT array(SELECT id FROM t LIMIT 2)",
    ] {
        let err = conn.query(sql).unwrap_err();
        assert!(
            format!("{err:?}").contains("not supported"),
            "{sql} must be rejected: {err:?}"
        );
    }
}
