use std::sync::Arc;

use turso_core::{schema::Table, Connection};
use turso_parser::ast;

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
    rewrite_one_select(conn, &mut select.body.select);
    for compound in &mut select.body.compounds {
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

    if let Some(where_clause) = where_clause.as_deref_mut() {
        rewrite_expr(conn, where_clause);
    }
    if let Some(from) = from.as_mut() {
        rewrite_from(conn, from);
    }

    if from.is_some() || columns.len() != 1 {
        return;
    }
    let ast::ResultColumn::Expr(expr, alias) = &columns[0] else {
        return;
    };
    let ast::Expr::FunctionCall { name, args, .. } = expr.as_ref() else {
        return;
    };
    let Some(first_column) = tvf_first_column(conn, name.as_str(), args.len()) else {
        return;
    };

    let call = ast::SelectTable::TableCall(
        ast::QualifiedName::single(name.clone()),
        args.clone(),
        None,
    );
    let projected = ast::Expr::Id(ast::Name::from_string(first_column));
    *columns = vec![ast::ResultColumn::Expr(Box::new(projected), alias.clone())];
    *from = Some(ast::FromClause {
        select: Box::new(call),
        joins: vec![],
    });
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
