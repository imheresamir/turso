use std::sync::Arc;

use turso_core::schema::Table;
use turso_core::Connection;
use turso_parser::ast;

/// Rewrite set-returning-function constructs that the core planner does not
/// support, into shapes it does.
///
/// A single, FROM-less, single-target-list call to a catalog table-valued
/// function (e.g. `pg_partition_ancestors('16384')') is exactly a `TableCall`
/// scan, so it is lifted into `SELECT <first visible column> FROM tvf(args)`.
/// TVF-ness is detected from the registered virtual table's hidden columns
/// (which are the call arguments) — no function-name list is duplicated.
pub fn rewrite_stmt(conn: &Arc<Connection>, stmt: &mut ast::Stmt) {
    match stmt {
        ast::Stmt::Select(select) => rewrite_select(conn, select),
        ast::Stmt::Insert { body, .. } => {
            if let ast::InsertBody::Select(select, _) = body {
                rewrite_select(conn, select);
            }
        }
        ast::Stmt::Update(update) => {
            if let Some(where_clause) = update.where_clause.as_deref_mut() {
                rewrite_expr(conn, where_clause);
            }
        }
        ast::Stmt::Delete { where_clause, .. } => {
            if let Some(where_clause) = where_clause.as_deref_mut() {
                rewrite_expr(conn, where_clause);
            }
        }
        _ => {}
    }
}

fn rewrite_select(conn: &Arc<Connection>, select: &mut ast::Select) {
    let ast::Select {
        body,
        order_by,
        limit,
        ..
    } = select;
    rewrite_body(conn, body);
    for col in order_by.iter_mut() {
        rewrite_expr(conn, &mut col.expr);
    }
    if let Some(limit) = limit {
        rewrite_expr(conn, &mut limit.expr);
        if let Some(offset) = &mut limit.offset {
            rewrite_expr(conn, offset);
        }
    }
}

fn rewrite_body(conn: &Arc<Connection>, body: &mut ast::SelectBody) {
    rewrite_one_select(conn, &mut body.select);
    for compound in &mut body.compounds {
        rewrite_one_select(conn, &mut compound.select);
    }
}

fn rewrite_one_select(conn: &Arc<Connection>, one: &mut ast::OneSelect) {
    let ast::OneSelect::Select {
        columns,
        from,
        where_clause,
        ..
    } = one
    else {
        return;
    };

    // Lift a FROM-less single-target-list TVF call into a TableCall scan.
    if from.is_none() && columns.len() == 1 {
        if let ast::ResultColumn::Expr(expr, alias) = &mut columns[0] {
            if let ast::Expr::FunctionCall { name, args, .. } = expr.as_ref() {
                if let Some(first_column) = tvf_first_column(conn, name.as_str(), args.len()) {
                    let call = ast::SelectTable::TableCall(
                        ast::QualifiedName::single(name.clone()),
                        args.clone(),
                        None,
                    );
                    let projected = ast::Expr::Id(ast::Name::from_string(first_column));
                    columns[0] = ast::ResultColumn::Expr(Box::new(projected), alias.clone());
                    *from = Some(ast::FromClause {
                        select: Box::new(call),
                        joins: vec![],
                    });
                    return;
                }
            }
        }
    }

    for column in columns.iter_mut() {
        if let ast::ResultColumn::Expr(expr, _) = column {
            rewrite_expr(conn, expr);
        }
    }
    if let Some(from) = from.as_mut() {
        rewrite_from(conn, from);
    }
    if let Some(where_clause) = where_clause.as_deref_mut() {
        rewrite_expr(conn, where_clause);
    }
}

fn rewrite_from(conn: &Arc<Connection>, from: &mut ast::FromClause) {
    rewrite_select_table(conn, &mut from.select);
    for join in &mut from.joins {
        rewrite_select_table(conn, &mut join.table);
    }
}

fn rewrite_select_table(conn: &Arc<Connection>, table: &mut ast::SelectTable) {
    match table {
        ast::SelectTable::Select(select, _) => rewrite_select(conn, select),
        ast::SelectTable::Sub(from, _) => rewrite_from(conn, from),
        _ => {}
    }
}

fn rewrite_expr(conn: &Arc<Connection>, expr: &mut ast::Expr) {
    match expr {
        ast::Expr::InSelect { lhs, rhs, .. } => {
            rewrite_expr(conn, lhs);
            rewrite_select(conn, rhs);
        }
        ast::Expr::Subquery(select) | ast::Expr::Exists(select) => rewrite_select(conn, select),
        ast::Expr::Binary(lhs, _, rhs) => {
            rewrite_expr(conn, lhs);
            rewrite_expr(conn, rhs);
        }
        ast::Expr::Unary(_, inner)
        | ast::Expr::IsNull(inner)
        | ast::Expr::NotNull(inner)
        | ast::Expr::Collate(inner, _)
        | ast::Expr::Cast { expr: inner, .. } => rewrite_expr(conn, inner),
        ast::Expr::Parenthesized(exprs) => {
            for inner in exprs {
                rewrite_expr(conn, inner);
            }
        }
        ast::Expr::InList { lhs, rhs, .. } => {
            rewrite_expr(conn, lhs);
            for inner in rhs {
                rewrite_expr(conn, inner);
            }
        }
        _ => {}
    }
}

fn tvf_first_column(conn: &Arc<Connection>, name: &str, arg_count: usize) -> Option<String> {
    let schema = conn.current_schema();
    let table = schema.get_table(name)?;
    let Table::Virtual(vtab) = table.as_ref() else {
        return None;
    };
    let hidden = vtab.columns().iter().filter(|c| c.hidden()).count();
    if hidden == 0 || arg_count > hidden {
        return None;
    }
    vtab.columns()
        .iter()
        .find(|column| !column.hidden())
        .and_then(|column| column.name.clone())
}
