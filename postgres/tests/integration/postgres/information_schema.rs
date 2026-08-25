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

// `table_constraints` must report a PRIMARY KEY only for tables that
// actually have one. A table without a PK must not be fabricated as having one.
#[turso_macros::test]
fn test_pg_information_schema_table_constraints_reports_only_real_pks(db: TempDatabase) {
    let conn = db.connect_postgres();

    conn.execute("CREATE TABLE haspk (id integer PRIMARY KEY, name text)")
        .unwrap();
    conn.execute("CREATE TABLE nopk (a text, b text)").unwrap();

    // The PK'd table appears in table_constraints (constraint_type is the
    // second column; `query_text` reads column 0, so assert on table_name).
    let out = query_text(
        &conn,
        "SELECT table_name FROM information_schema.table_constraints \
         WHERE table_name = 'haspk'",
    );
    assert_eq!(out, ["haspk"]);

    // And the constraint_type is PRIMARY KEY (read column 1 explicitly).
    let mut rows = conn
        .query(
            "SELECT constraint_type FROM information_schema.table_constraints \
             WHERE table_name = 'haspk'",
        )
        .unwrap()
        .unwrap();
    loop {
        match rows.step().unwrap() {
            StepResult::Row => {
                assert_eq!(
                    *rows.row().unwrap().get_value(0),
                    Value::Text("PRIMARY KEY".into())
                );
            }
            StepResult::Done => break,
            _ => {}
        }
    }

    // The PK-less table is omitted entirely — no fabricated constraint.
    let out = query_text(
        &conn,
        "SELECT table_name FROM information_schema.table_constraints \
         WHERE table_name = 'nopk'",
    );
    assert!(
        out.is_empty(),
        "nopk must not report a fabricated PK: {out:?}"
    );
}
