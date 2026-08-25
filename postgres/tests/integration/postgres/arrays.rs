
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
