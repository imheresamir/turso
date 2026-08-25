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
fn test_pg_type_reports_internal_typnames(db: TempDatabase) {
    let conn = db.connect_postgres();

    // PostgreSQL stores the internal typname in pg_type (e.g. "int4", not the
    // canonical "integer"). Clients and the type-input parser resolve the
    // canonical alias via format_type / regtype, not a typname row.
    for (internal, oid) in [
        ("int4", 23),
        ("int8", 20),
        ("int2", 21),
        ("float4", 700),
        ("float8", 701),
        ("text", 25),
    ] {
        let sql = format!("SELECT oid FROM pg_type WHERE typname = '{internal}'");
        let oids = query_integer(&conn, &sql);
        assert_eq!(
            oids.len(),
            1,
            "pg_type must contain internal typname '{internal}'"
        );
        assert_eq!(oids[0], oid, "wrong OID for internal typname '{internal}'");
    }
}

#[turso_macros::test]
fn test_pg_type_canonical_via_format_type(db: TempDatabase) {
    let conn = db.connect_postgres();

    // The canonical type name is surfaced through format_type (the PostgreSQL
    // display channel), not pg_type.typname. oid 23 -> "integer", 20 -> "bigint".
    for (oid, canonical) in [(23, "integer"), (20, "bigint"), (21, "smallint")] {
        let sql = format!("SELECT format_type({oid}, -1)");
        let names = query_text(&conn, &sql);
        assert_eq!(names.len(), 1, "format_type({oid}, -1) must return a row");
        assert_eq!(
            names[0], canonical,
            "format_type({oid}, -1) should return canonical '{canonical}'"
        );
    }
}
