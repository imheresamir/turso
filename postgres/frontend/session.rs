use std::collections::HashMap;
use std::num::NonZero;
use std::path::PathBuf;
use std::str;
use std::sync::{Arc, Mutex};

use crate::aliases;
use crate::catalog::{self, PostgresDialect};
use turso_core::{CheckpointMode, Connection, IOResult, LimboError, PrepareOptions, Result, Statement, Value};
use turso_core::return_if_io;
use turso_parser::ast::{self};
use turso_pg_parser::translator::{
    is_comment_on, is_refresh_matview, try_extract_copy_from, try_extract_create_schema,
    try_extract_deallocate, try_extract_drop_schema, try_extract_execute, try_extract_explain,
    try_extract_prepare, try_extract_set, try_extract_show, PgCopyFromStmt, PgCreateSchemaStmt,
    PgDeallocateStmt, PgDropSchemaStmt, PgExplainStmt, PgPrepareStmt, PgSetStmt,
    PostgreSQLTranslator,
};

use crate::copy::parse_copy_text_format;

#[derive(Clone)]
pub struct PgConnection {
    inner: Arc<PgConnectionInner>,
}

impl PgConnection {
    pub fn checkpoint(&self) -> Result<()> {
        self.inner
            .conn
            .checkpoint(CheckpointMode::Truncate {
                upper_bound_inclusive: None,
            })
            .map(|_| ())
    }
    pub fn checkpoint_schemas(&self) -> Result<()> {
        self.inner
            .conn
            .checkpoint_attached(CheckpointMode::Truncate {
                upper_bound_inclusive: None,
            })
            .map(|_| ())
    }
    pub fn set_schema_dir(&self, dir: PathBuf) {
        *self.inner.schema_dir.lock().unwrap() = Some(dir);
    }
    pub fn set_schema_storage_factory(
        &self,
        f: Arc<
            dyn Fn(&str, &std::path::Path) -> Result<Arc<dyn turso_core::storage::database::DatabaseStorage>>
                + Send
                + Sync,
        >,
    ) {
        *self.inner.schema_storage_factory.lock().unwrap() = Some(f);
    }
    pub fn set_schema_drop(&self, f: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.inner.schema_drop.lock().unwrap() = Some(f);
    }
    /// Register a closure reporting the backing celld cell's on-disk byte size.
    /// The PostgreSQL compat layer uses this for `pg_table_size` and friends,
    /// which have no per-relation size in the single-file model.
    pub fn set_relation_size_fn(&self, f: Arc<dyn Fn() -> i64 + Send + Sync>) {
        self.inner.conn.set_relation_size_fn(f);
    }

    /// Register the authenticated role for the session. `current_user` /
    /// `session_user` / `user` / `current_role` surface this value.
    pub fn set_current_user(&self, user: String) {
        self.inner.conn.set_current_user(user);
    }
}

struct PgConnectionInner {
    conn:          Arc<Connection>,
    session_state: Mutex<SessionState>,
    schema_dir:    Mutex<Option<PathBuf>>,
    schema_storage_factory: Mutex<
        Option<
            Arc<
                dyn Fn(
                    &str,
                    &std::path::Path,
                )
                    -> Result<Arc<dyn turso_core::storage::database::DatabaseStorage>>
                    + Send
                    + Sync,
            >,
        >,
    >,
    schema_drop: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl PgConnectionInner {
    fn set_search_path(&self, path: Vec<String>) {
        let mut state = self.session_state.lock().unwrap();
        state.search_path = path;
    }
}

#[derive(Default)]
struct SessionState {
    search_path: Vec<String>,
    prepared: HashMap<String, String>,
}

/// Open a database with the PostgreSQL schema dialect, resolving the IO
/// backend from `vfs` or the path like [`turso_core::Database::open_new`].
pub fn open_database(
    path: &str,
    vfs: Option<&str>,
    flags: turso_core::OpenFlags,
    opts: turso_core::DatabaseOpts,
) -> Result<(Arc<dyn turso_core::IO>, Arc<turso_core::Database>)> {
    let io = match vfs {
        Some(vfs) => turso_core::Database::io_for_vfs(vfs)?,
        None => turso_core::Database::io_for_path(path)?,
    };
    let db = open_database_with_io(io.clone(), path, flags, opts)?;
    Ok((io, db))
}

/// Open a database with the PostgreSQL schema dialect on an existing IO
/// backend.
pub fn open_database_with_io(
    io: Arc<dyn turso_core::IO>,
    path: &str,
    flags: turso_core::OpenFlags,
    opts: turso_core::DatabaseOpts,
) -> Result<Arc<turso_core::Database>> {
    let file = io.open_file(path, flags, true)?;
    let db_file = Arc::new(turso_core::storage::database::DatabaseFile::new(file));
    turso_core::Database::open(
        io,
        path,
        turso_core::OpenOptions::new(Arc::new(PostgresDialect))
            .storage(db_file)
            .flags(flags)
            .db_opts(opts),
    )
}

/// Open a database with the PostgreSQL schema dialect on an existing IO
/// backend, using a caller-provided page storage backend instead of Turso's
/// built-in file storage.
///
/// This is the storage-injection seam that lets embedders back the PostgreSQL
/// frontend with a custom `DatabaseStorage` (e.g. an in-memory or remote KV
/// store) while still registering the `pg_catalog` virtual tables that
/// `PostgresDialect::register_catalog` installs. `open_database_with_io`
/// hardcodes `DatabaseFile` storage and cannot be used with a custom backend.
pub fn open_database_with_storage(
    io: Arc<dyn turso_core::IO>,
    path: &str,
    flags: turso_core::OpenFlags,
    opts: turso_core::DatabaseOpts,
    storage: Arc<dyn turso_core::storage::database::DatabaseStorage>,
) -> Result<Arc<turso_core::Database>> {
    turso_core::Database::open(
        io,
        path,
        turso_core::OpenOptions::new(Arc::new(PostgresDialect))
            .storage(storage)
            .flags(flags)
            .db_opts(opts),
    )
}

impl PgConnection {
    pub fn new(conn: Arc<Connection>) -> Self {
        aliases::install(&conn);
        Self {
            inner: Arc::new(PgConnectionInner {
                conn,
                session_state: Mutex::new(SessionState::default()),
                schema_dir: Mutex::new(None),
                schema_storage_factory: Mutex::new(None),
                schema_drop: Mutex::new(None),
            }),
        }
    }

    pub fn inner(&self) -> &Arc<Connection> {
        &self.inner.conn
    }

    pub fn prepare(&self, sql: impl AsRef<str>) -> Result<Statement> {
        prepare_statement(&self.inner, sql.as_ref())
    }

    pub fn query(&self, sql: impl AsRef<str>) -> Result<Option<Statement>> {
        let sql = sql.as_ref().trim();
        if sql.is_empty() {
            return Ok(None);
        }
        self.prepare(sql).map(Some)
    }

    pub fn execute(&self, sql: impl AsRef<str>) -> Result<()> {
        for stmt in self.query_runner(sql.as_ref().as_bytes()) {
            if let Some(mut stmt) = stmt? {
                stmt.run_ignore_rows()?;
            }
        }
        Ok(())
    }

    pub fn close(&self) -> Result<()> {
        self.inner.conn.close()
    }

    pub fn pragma_update(&self, name: &str, value: impl std::fmt::Display) -> Result<()> {
        let sql = format!("PRAGMA {name} = {value}");
        let mut stmt = self.inner.conn.prepare_internal(sql)?;
        stmt.run_ignore_rows()
    }

    pub fn query_runner<'a>(&'a self, sql: &'a [u8]) -> PgQueryRunner<'a> {
        PgQueryRunner::new(&self.inner, sql)
    }
}

pub struct PgQueryRunner<'a> {
    conn: &'a Arc<PgConnectionInner>,
    stmts: Vec<String>,
    index: usize,
}

impl<'a> PgQueryRunner<'a> {
    fn new(conn: &'a Arc<PgConnectionInner>, sql: &'a [u8]) -> Self {
        let sql = str::from_utf8(sql).unwrap_or("");
        Self {
            conn,
            stmts: split_statements(sql)
                .unwrap_or_else(|_| vec![sql.trim().to_string()])
                .into_iter()
                .filter(|stmt| !stmt.trim().is_empty())
                .collect(),
            index: 0,
        }
    }
}

impl Iterator for PgQueryRunner<'_> {
    type Item = Result<Option<Statement>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.stmts.len() {
            return None;
        }

        let sql = &self.stmts[self.index];
        self.index += 1;
        Some(prepare_statement(self.conn, sql).map(Some))
    }
}

pub fn split_statements(sql: &str) -> Result<Vec<String>> {
    match turso_pg_parser::split_statements(sql) {
        Ok(stmts) if stmts.is_empty() && !sql.trim().is_empty() => Ok(vec![sql.trim().to_string()]),
        Ok(stmts) => Ok(stmts),
        Err(_) => Ok(vec![sql.trim().to_string()]),
    }
}

fn prepare_statement(pg_conn: &Arc<PgConnectionInner>, sql: &str) -> Result<Statement> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err(LimboError::InvalidArgument(
            "The supplied SQL string contains no statements".to_string(),
        ));
    }

    reject_sqlite_catalog_access(sql)?;

    if let Some(stmt) = try_prepare_special(pg_conn, sql)? {
        return Ok(stmt);
    }

    let parse_result =
        turso_pg_parser::parse(sql).map_err(|e| LimboError::ParseError(e.to_string()))?;
    let translator = PostgreSQLTranslator::new();
    let translated = translator
        .translate_with_prereqs(&parse_result)
        .map_err(|e| LimboError::ParseError(e.to_string()))?;
    reject_catalog_dml(&translated.stmt)?;

    let options = {
        let state = pg_conn.session_state.lock().unwrap();
        let path = state.search_path.clone();
        PrepareOptions {
            unqualified_database_search_path: if path.is_empty() { None } else { Some(path) },
        }
    };
    for prereq in translated.prereqs {
        let input = prereq.to_string();
        let mut stmt = pg_conn
            .conn
            .prepare_translated_stmt_with_options(prereq, &input, &options)?;
        stmt.run_ignore_rows()?;
    }

    let mut stmt = translated.stmt;
    crate::srf::rewrite_stmt(&pg_conn.conn, &mut stmt);
    pg_conn
        .conn
        .prepare_translated_stmt_with_options(stmt, sql, &options)
}

fn reject_catalog_dml(stmt: &ast::Stmt) -> Result<()> {
    let table_name = match stmt {
        ast::Stmt::Insert { tbl_name, .. } => Some(tbl_name.name.as_str()),
        ast::Stmt::Delete { tbl_name, .. } => Some(tbl_name.name.as_str()),
        ast::Stmt::Update(update) => Some(update.tbl_name.name.as_str()),
        _ => None,
    };

    let Some(table_name) = table_name else {
        return Ok(());
    };

    if !catalog::is_catalog_table_name(table_name) {
        return Ok(());
    }

    let verb = match stmt {
        ast::Stmt::Insert { .. } => "insert into",
        ast::Stmt::Delete { .. } => "delete from",
        ast::Stmt::Update { .. } => "update",
        _ => unreachable!(),
    };
    Err(LimboError::ParseError(format!(
        "cannot {verb} pg_catalog table \"{table_name}\""
    )))
}

fn reject_sqlite_catalog_access(sql: &str) -> Result<()> {
    let lower = sql.to_ascii_lowercase();
    for table_name in ["sqlite_master", "sqlite_schema"] {
        if lower.contains(table_name) {
            return Err(LimboError::ParseError(format!(
                "no such table: {table_name}"
            )));
        }
    }
    Ok(())
}

fn try_prepare_special(pg_conn: &Arc<PgConnectionInner>, sql: &str) -> Result<Option<Statement>> {
    let parse_result = match turso_pg_parser::parse(sql) {
        Ok(result) => result,
        Err(_) => return Ok(None),
    };

    if let Some(set_stmt) = try_extract_set(&parse_result) {
        let stmt = handle_pg_set(pg_conn, &set_stmt)?;
        return Ok(Some(stmt));
    }

    if let Some(show_stmt) = try_extract_show(&parse_result) {
        let pragma_sql = format!("PRAGMA {}", show_stmt.name);
        return Ok(Some(pg_conn.conn.prepare(&pragma_sql)?));
    }

    if let Some(stmt) = try_extract_create_schema(&parse_result) {
        handle_pg_create_schema(pg_conn, &stmt)?;
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    if let Some(stmt) = try_extract_drop_schema(&parse_result) {
        handle_pg_drop_schema(pg_conn, &stmt)?;
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    if is_refresh_matview(&parse_result) {
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    if is_comment_on(&parse_result) {
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    if let Some(stmt) = try_extract_copy_from(&parse_result) {
        let rows_inserted = handle_pg_copy_from(&pg_conn.conn, &stmt)?;
        let stmt = noop_statement(&pg_conn.conn)?;
        stmt.set_n_change(rows_inserted as i64);
        return Ok(Some(stmt));
    }

    if let Some(stmt) = try_extract_explain(&parse_result) {
        let inner = prepare_explained_stmt(pg_conn, &stmt)?;
        return Ok(Some(inner));
    }

    if let Some(stmt) = try_extract_prepare(&parse_result) {
        handle_pg_prepare(pg_conn, &stmt)?;
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    if let Some(stmt) = try_extract_execute(&parse_result) {
        let mut prepared = prepare_statement(pg_conn, &lookup_prepared(pg_conn, &stmt.name)?)?;
        bind_executed_params(&mut prepared, &stmt.params)?;
        return Ok(Some(prepared));
    }

    if let Some(stmt) = try_extract_deallocate(&parse_result) {
        handle_pg_deallocate(pg_conn, &stmt)?;
        return Ok(Some(noop_statement(&pg_conn.conn)?));
    }

    Ok(None)
}

fn noop_statement(conn: &Arc<Connection>) -> Result<Statement> {
    conn.prepare("SELECT 0 WHERE 0")
}

fn prepare_explained_stmt(
    pg_conn: &Arc<PgConnectionInner>,
    explain: &PgExplainStmt,
) -> Result<Statement> {
    let sql = explain.query.trim();
    reject_sqlite_catalog_access(sql)?;

    let parse_result =
        turso_pg_parser::parse(sql).map_err(|e| LimboError::ParseError(e.to_string()))?;
    let translator = PostgreSQLTranslator::new();
    let translated = translator
        .translate_with_prereqs(&parse_result)
        .map_err(|e| LimboError::ParseError(e.to_string()))?;
    reject_catalog_dml(&translated.stmt)?;

    let options = {
        let state = pg_conn.session_state.lock().unwrap();
        let path = state.search_path.clone();
        PrepareOptions {
            unqualified_database_search_path: if path.is_empty() { None } else { Some(path) },
        }
    };
    for prereq in translated.prereqs {
        let input = prereq.to_string();
        let mut stmt = pg_conn
            .conn
            .prepare_translated_stmt_with_options(prereq, &input, &options)?;
        stmt.run_ignore_rows()?;
    }

    let cmd = if explain.analyze {
        ast::Cmd::ExplainQueryPlan(translated.stmt)
    } else {
        ast::Cmd::Explain(translated.stmt)
    };
    pg_conn
        .conn
        .prepare_translated_cmd_with_options(cmd, sql, &options)
}

fn execute_sqlite_internal(conn: &Arc<Connection>, sql: impl AsRef<str>) -> Result<()> {
    let mut stmt = conn.prepare_internal(sql)?;
    stmt.run_ignore_rows()
}

fn handle_pg_set(pg_conn: &Arc<PgConnectionInner>, set_stmt: &PgSetStmt) -> Result<Statement> {
    if set_stmt.name == "search_path" {
        let path = set_stmt
            .values
            .iter()
            .map(|value| value.as_search_path_name().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| LimboError::ParseError("incorrect format".to_string()))?;
        pg_conn.set_search_path(path);
        return noop_statement(&pg_conn.conn);
    }
    let value = set_stmt.values.first().ok_or_else(|| {
        LimboError::ParseError(format!("SET {}: no value provided", set_stmt.name))
    })?;
    let pragma_sql = format!("PRAGMA {} = {}", set_stmt.name, value.to_sql_string());
    pg_conn.conn.prepare(&pragma_sql)
}

fn handle_pg_prepare(pg_conn: &Arc<PgConnectionInner>, stmt: &PgPrepareStmt) -> Result<()> {
    let mut state = pg_conn.session_state.lock().unwrap();
    state.prepared.insert(stmt.name.clone(), stmt.query.clone());
    Ok(())
}

fn lookup_prepared(pg_conn: &Arc<PgConnectionInner>, name: &str) -> Result<String> {
    let state = pg_conn.session_state.lock().unwrap();
    state
        .prepared
        .get(name)
        .cloned()
        .ok_or_else(|| LimboError::ParseError(format!("prepared statement \"{name}\" does not exist")))
}

fn handle_pg_deallocate(pg_conn: &Arc<PgConnectionInner>, stmt: &PgDeallocateStmt) -> Result<()> {
    let mut state = pg_conn.session_state.lock().unwrap();
    match &stmt.name {
        Some(name) => {
            state.prepared.remove(name);
        }
        None => state.prepared.clear(),
    }
    Ok(())
}

fn bind_executed_params(stmt: &mut Statement, params: &[String]) -> Result<()> {
    for (i, raw) in params.iter().enumerate() {
        let value = literal_to_value(raw);
        let pg_param_name = format!("${}", i + 1);
        let idx = stmt
            .parameter_index(&pg_param_name)
            .unwrap_or_else(|| NonZero::new(i + 1).expect("parameter index must be non-zero"));
        let _ = stmt.bind_at(idx, value);
    }
    Ok(())
}

fn literal_to_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("NULL") {
        return Value::Null;
    }
    if trimmed.eq_ignore_ascii_case("true") {
        return Value::from_i64(1);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return Value::from_i64(0);
    }
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        return Value::Text(inner.replace("''", "'").into());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Value::from_i64(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Value::from_f64(f);
    }
    Value::Text(trimmed.to_string().into())
}

fn handle_pg_create_schema(pg_conn: &Arc<PgConnectionInner>, stmt: &PgCreateSchemaStmt) -> Result<()> {
    let name = stmt.name.to_lowercase();
    if name == "public" {
        if stmt.if_not_exists {
            return Ok(());
        }
        return Err(LimboError::ParseError(format!(
            "schema \"{name}\" already exists"
        )));
    }

    if schema_exists(&pg_conn.conn, &name)? {
        if stmt.if_not_exists {
            return Ok(());
        }
        return Err(LimboError::ParseError(format!(
            "schema \"{name}\" already exists"
        )));
    }

    let path = schema_file_path(pg_conn, &name);
    if let Some(factory) = &*pg_conn.schema_storage_factory.lock().unwrap() {
        let path_ref: &std::path::Path = std::path::Path::new(&path);
        let storage = factory(&name, path_ref)?;
        pg_conn.conn.attach_database_with_storage(&path, &name, storage)?;
        return Ok(());
    }
    execute_sqlite_internal(
        &pg_conn.conn,
        format!("ATTACH '{}' AS \"{}\"", path.replace('\'', "''"), name),
    )?;
    Ok(())
}

fn schema_file_path(pg_conn: &Arc<PgConnectionInner>, schema_name: &str) -> String {
    let filename = format!("turso-postgres-schema-{schema_name}.db");
    if let Some(dir) = &*pg_conn.schema_dir.lock().unwrap() {
        return dir.join(&filename).to_string_lossy().to_string();
    }
    let main_path = pg_conn.conn.db_file_path();
    if main_path == ":memory:" {
        filename
    } else {
        let parent = std::path::Path::new(&main_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        parent.join(&filename).to_string_lossy().to_string()
    }
}

fn handle_pg_drop_schema(pg_conn: &Arc<PgConnectionInner>, stmt: &PgDropSchemaStmt) -> Result<()> {
    let name = stmt.name.to_lowercase();
    if name == "public" {
        return handle_pg_drop_schema_public(&pg_conn.conn, stmt.cascade);
    }

    if !schema_exists(&pg_conn.conn, &name)? {
        if stmt.if_exists {
            return Ok(());
        }
        return Err(LimboError::ParseError(format!(
            "schema \"{name}\" does not exist"
        )));
    }

    if stmt.cascade {
        drop_all_tables_in_schema(&pg_conn.conn, &name)?;
    }

    execute_sqlite_internal(&pg_conn.conn, format!("DETACH \"{name}\""))?;
    if let Some(drop) = &*pg_conn.schema_drop.lock().unwrap() {
        drop(&name);
    }
    Ok(())
}

fn handle_pg_drop_schema_public(conn: &Arc<Connection>, cascade: bool) -> Result<()> {
    let table_names = list_user_tables(conn, None)?;
    if !cascade && !table_names.is_empty() {
        return Err(LimboError::ParseError(
            "cannot drop schema \"public\" because other objects depend on it".to_string(),
        ));
    }

    for table_name in table_names {
        let mut stmt = conn.prepare(format!("DROP TABLE \"{table_name}\""))?;
        stmt.run_ignore_rows()?;
    }
    Ok(())
}

fn drop_all_tables_in_schema(conn: &Arc<Connection>, schema_name: &str) -> Result<()> {
    for table_name in list_user_tables(conn, Some(schema_name))? {
        let mut stmt = conn.prepare(format!("DROP TABLE \"{schema_name}\".\"{table_name}\"",))?;
        stmt.run_ignore_rows()?;
    }
    Ok(())
}

fn handle_pg_copy_from(conn: &Arc<Connection>, stmt: &PgCopyFromStmt) -> Result<usize> {
    let data = std::fs::read_to_string(&stmt.filename).map_err(|e| {
        LimboError::ParseError(format!("COPY FROM: cannot read '{}': {}", stmt.filename, e))
    })?;

    let table_name = match &stmt.schema_name {
        Some(schema) => format!("\"{schema}\".\"{}\"", stmt.table_name),
        None => format!("\"{}\"", stmt.table_name),
    };
    let column_names = get_table_columns(conn, &stmt.table_name, stmt.schema_name.as_deref())?;
    if column_names.is_empty() {
        return Err(LimboError::ParseError(format!(
            "COPY FROM: table '{}' not found or has no columns",
            stmt.table_name
        )));
    }

    let (insert_cols, num_columns) = match &stmt.columns {
        Some(cols) => {
            let col_list = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            (format!(" ({col_list})"), cols.len())
        }
        None => (String::new(), column_names.len()),
    };

    let placeholders = (0..num_columns).map(|_| "?").collect::<Vec<_>>().join(", ");
    let insert_sql = format!("INSERT INTO {table_name}{insert_cols} VALUES ({placeholders})");

    let delimiter = stmt
        .delimiter
        .as_ref()
        .and_then(|d| d.chars().next())
        .unwrap_or('\t');
    let null_string = stmt.null_string.as_deref().unwrap_or("\\N");

    let mut rows = parse_copy_text_format(&data, delimiter, null_string, num_columns)?;
    if stmt.header && !rows.is_empty() {
        rows.remove(0);
    }

    let rows_inserted = rows.len();
    let mut begin = conn.prepare_sqlite("BEGIN")?;
    begin.run_ignore_rows()?;

    let result = (|| {
        let mut insert_stmt = conn.prepare_sqlite(&insert_sql)?;
        for row in &rows {
            for (i, val) in row.iter().enumerate() {
                let index = NonZero::new(i + 1).unwrap();
                match val {
                    Some(s) => insert_stmt.bind_at(index, Value::build_text(s.clone()))?,
                    None => insert_stmt.bind_at(index, Value::Null)?,
                }
            }
            insert_stmt.run_ignore_rows()?;
            insert_stmt.reset()?;
            insert_stmt.clear_bindings();
        }

        let mut commit = conn.prepare_sqlite("COMMIT")?;
        commit.run_ignore_rows()?;
        Ok(rows_inserted)
    })();

    if result.is_err() {
        if let Ok(mut rollback) = conn.prepare_sqlite("ROLLBACK") {
            let _ = rollback.run_ignore_rows();
        }
    }

    result
}

fn get_table_columns(
    conn: &Arc<Connection>,
    table_name: &str,
    schema_name: Option<&str>,
) -> Result<Vec<String>> {
    let sql = match schema_name {
        Some(schema) => format!("PRAGMA \"{schema}\".table_info('{table_name}')"),
        None => format!("PRAGMA table_info('{table_name}')"),
    };
    let mut stmt = conn.prepare_internal(&sql)?;
    let rows = stmt.run_collect_rows()?;
    Ok(rows
        .into_iter()
        .filter_map(|row| match row.get(1) {
            Some(Value::Text(t)) => Some(t.as_str().to_string()),
            _ => None,
        })
        .collect())
}

fn list_user_tables(conn: &Arc<Connection>, schema_name: Option<&str>) -> Result<Vec<String>> {
    let filter = "type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '__turso_internal_%'";
    let sql = match schema_name {
        Some(name) => format!("SELECT name FROM \"{name}\".sqlite_schema WHERE {filter}"),
        None => format!("SELECT name FROM sqlite_schema WHERE {filter}"),
    };
    let mut stmt = conn.prepare_internal(&sql)?;
    let rows = stmt.run_collect_rows()?;
    Ok(rows
        .into_iter()
        .filter_map(|row| match row.first() {
            Some(Value::Text(t)) => Some(t.as_str().to_string()),
            _ => None,
        })
        .collect())
}

fn schema_exists(conn: &Arc<Connection>, schema_name: &str) -> Result<bool> {
    let sql = format!(
        "SELECT 1 FROM pragma_database_list WHERE name = '{}'",
        schema_name.replace('\'', "''")
    );
    let mut stmt = conn.prepare_internal(&sql)?;
    let rows = stmt.run_collect_rows()?;
    Ok(!rows.is_empty())
}
