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
fn test_pg_information_schema_tables(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE widgets (id integer PRIMARY KEY, name text)")
        .unwrap();

    let out = query_text(
        &conn,
        "SELECT table_name FROM information_schema.tables WHERE table_name = 'widgets'",
    );
    assert_eq!(out, ["widgets"]);
}

#[turso_macros::test]
fn test_pg_information_schema_columns(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.execute("CREATE TABLE widgets (id integer PRIMARY KEY, name text)")
        .unwrap();

    let out = query_text(
        &conn,
        "SELECT column_name FROM information_schema.columns WHERE table_name = 'widgets' ORDER BY ordinal_position",
    );
    assert_eq!(out, ["id", "name"]);
}

#[turso_macros::test]
fn test_pg_information_schema_schemata(db: TempDatabase) {
    let conn = db.connect_postgres();

    let out = query_text(
        &conn,
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name = 'public'",
    );
    assert_eq!(out, ["public"]);
}
