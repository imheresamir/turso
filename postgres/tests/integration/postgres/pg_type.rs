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
fn test_pg_type_reports_canonical_typnames(db: TempDatabase) {
    let conn = db.connect_postgres();

    // The pg_type virtual table must surface PostgreSQL's canonical type
    // names, not Turso's internal ones (int4 -> integer, int8 -> bigint, ...).
    for (internal, canonical) in [
        ("integer", "integer"),
        ("bigint", "bigint"),
        ("smallint", "smallint"),
        ("real", "real"),
        ("double precision", "double precision"),
    ] {
        let sql = format!("SELECT count(*) FROM pg_type WHERE typname = '{canonical}'");
        let count = query_integer(&conn, &sql);
        assert_eq!(count.len(), 1);
        assert!(
            count[0] >= 1,
            "pg_type must report canonical typname '{canonical}' (looked up via '{internal}')"
        );
    }
}
