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
fn test_pg_current_user_returns_set_role(db: TempDatabase) {
    let conn = db.connect_postgres();
    conn.set_current_user("alice".to_string());

    assert_eq!(query_text(&conn, "SELECT current_user"), ["alice"]);
    assert_eq!(query_text(&conn, "SELECT session_user"), ["alice"]);
    assert_eq!(query_text(&conn, "SELECT user"), ["alice"]);
    assert_eq!(query_text(&conn, "SELECT current_role"), ["alice"]);
}

#[turso_macros::test]
fn test_pg_current_user_default(db: TempDatabase) {
    let conn = db.connect_postgres();

    let out = query_text(&conn, "SELECT current_user");
    assert_eq!(out.len(), 1);
    assert!(!out[0].is_empty(), "current_user must not be empty");
}
