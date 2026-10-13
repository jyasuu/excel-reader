//! Executing a parsed [`Query`] against a [`Table`].

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::sql::{AggArg, AggFunc, CmpOp, Expr, Literal, Operand, OrderKey, Query, SelectExpr};
use crate::table::Table;
use crate::value::Value;

/// Run a query against a table and return the resulting table.
pub fn execute(table: &Table, query: &Query) -> Result<Table> {
    let filtered = apply_filter(table, query.filter.as_ref())?;

    let aggregated = !query.group_by.is_empty()
        || query
            .select
            .iter()
            .any(|item| matches!(item.expr, SelectExpr::Aggregate { .. }));

    let built = if aggregated {
        aggregate(table, query, &filtered)?
    } else {
        project(table, query, &filtered)?
    };

    let ordered = apply_order(table, built, &query.order_by)?;

    let mut rows = ordered.rows;
    if let Some(limit) = query.limit {
        rows.truncate(limit);
    }

    Ok(Table { columns: ordered.columns, rows })
}

// ---------------------------------------------------------------------------
// WHERE
// ---------------------------------------------------------------------------

fn apply_filter(table: &Table, filter: Option<&Expr>) -> Result<Vec<usize>> {
    let mut kept = Vec::new();
    for (i, row) in table.rows.iter().enumerate() {
        let keep = match filter {
            Some(expr) => eval(expr, table, row)?,
            None => true,
        };
        if keep {
            kept.push(i);
        }
    }
    Ok(kept)
}

fn eval(expr: &Expr, table: &Table, row: &[Value]) -> Result<bool> {
    Ok(match expr {
        Expr::And(a, b) => eval(a, table, row)? && eval(b, table, row)?,
        Expr::Or(a, b) => eval(a, table, row)? || eval(b, table, row)?,
        Expr::Not(inner) => !eval(inner, table, row)?,
        Expr::IsNull { operand, negated } => {
            let value = operand_value(operand, table, row)?;
            value.is_null() != *negated
        }
        Expr::Comparison { left, op, right } => {
            let l = operand_value(left, table, row)?;
            let r = operand_value(right, table, row)?;
            compare(&l, *op, &r)
        }
    })
}

/// Compare two values with SQL semantics: any comparison involving NULL is
/// unknown, which we treat as false.
fn compare(left: &Value, op: CmpOp, right: &Value) -> bool {
    if left.is_null() || right.is_null() {
        return false;
    }
    let ord = left.compare(right);
    match op {
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
    }
}

fn operand_value(operand: &Operand, table: &Table, row: &[Value]) -> Result<Value> {
    match operand {
        Operand::Column(name) => Ok(row[col_index(table, name)?].clone()),
        Operand::Literal(lit) => Ok(match lit {
            Literal::Number(n) => Value::Number(*n),
            Literal::Text(t) => Value::Text(t.clone()),
            Literal::Bool(b) => Value::Bool(*b),
            Literal::Null => Value::Null,
        }),
    }
}

// ---------------------------------------------------------------------------
// Projection (no aggregation)
// ---------------------------------------------------------------------------

struct Built {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    /// For plain projection: the source row index for each output row. `None`
    /// for aggregated results, which have no single source row.
    source: Option<Vec<usize>>,
}

fn project(table: &Table, query: &Query, filtered: &[usize]) -> Result<Built> {
    // Map each output column to a source column index.
    let mut plan: Vec<(String, usize)> = Vec::new();
    for item in &query.select {
        match &item.expr {
            SelectExpr::Star => {
                for (i, name) in table.columns.iter().enumerate() {
                    plan.push((name.clone(), i));
                }
            }
            SelectExpr::Column(name) => {
                let idx = col_index(table, name)?;
                let header = item.alias.clone().unwrap_or_else(|| name.clone());
                plan.push((header, idx));
            }
            SelectExpr::Aggregate { func, .. } => {
                bail!("aggregate {} cannot be used here", func.name())
            }
        }
    }

    if plan.is_empty() {
        bail!("the SELECT list is empty");
    }

    let columns: Vec<String> = plan.iter().map(|(name, _)| name.clone()).collect();
    let rows: Vec<Vec<Value>> = filtered
        .iter()
        .map(|&src| plan.iter().map(|(_, col)| table.rows[src][*col].clone()).collect())
        .collect();

    Ok(Built { columns, rows, source: Some(filtered.to_vec()) })
}

// ---------------------------------------------------------------------------
// GROUP BY / aggregates
// ---------------------------------------------------------------------------

/// Hashable, order-independent view of a set of group-key values.
#[derive(PartialEq, Eq, Hash, Clone)]
enum GroupKey {
    Null,
    Bool(bool),
    Num(u64),
    Text(String),
}

fn group_key(value: &Value) -> GroupKey {
    match value {
        Value::Null => GroupKey::Null,
        Value::Bool(b) => GroupKey::Bool(*b),
        Value::Number(n) => {
            let n = if *n == 0.0 { 0.0 } else { *n };
            GroupKey::Num(n.to_bits())
        }
        Value::Text(t) => GroupKey::Text(t.clone()),
    }
}

fn aggregate(table: &Table, query: &Query, filtered: &[usize]) -> Result<Built> {
    let group_cols: Vec<usize> = query
        .group_by
        .iter()
        .map(|name| col_index(table, name))
        .collect::<Result<_>>()?;

    // Bucket the filtered rows by group key, preserving first-seen order.
    let mut buckets: Vec<(Vec<Value>, Vec<usize>)> = Vec::new();
    let mut seen: HashMap<Vec<GroupKey>, usize> = HashMap::new();
    for &src in filtered {
        let key: Vec<Value> =
            group_cols.iter().map(|&c| table.rows[src][c].clone()).collect();
        let hash_key: Vec<GroupKey> = key.iter().map(group_key).collect();
        match seen.get(&hash_key) {
            Some(&i) => buckets[i].1.push(src),
            None => {
                seen.insert(hash_key, buckets.len());
                buckets.push((key, vec![src]));
            }
        }
    }

    // With aggregates but no GROUP BY, SQL produces exactly one row.
    if buckets.is_empty() && query.group_by.is_empty() {
        buckets.push((Vec::new(), Vec::new()));
    }

    // Build the output plan.
    enum Out {
        Key(usize),
        Agg { func: AggFunc, arg: Option<usize> },
    }
    let mut plan: Vec<(String, Out)> = Vec::new();
    for item in &query.select {
        match &item.expr {
            SelectExpr::Star => {
                if query.group_by.is_empty() {
                    bail!(
                        "SELECT * cannot be combined with aggregate functions; \
                         list the grouped columns explicitly or add a GROUP BY"
                    );
                }
                for (k, name) in query.group_by.iter().enumerate() {
                    plan.push((name.clone(), Out::Key(k)));
                }
            }
            SelectExpr::Column(name) => {
                let idx = col_index(table, name)?;
                let pos = group_cols.iter().position(|&c| c == idx).with_context(|| {
                    format!(
                        "column `{name}` must appear in GROUP BY or be wrapped in an \
                         aggregate function"
                    )
                })?;
                let header = item.alias.clone().unwrap_or_else(|| name.clone());
                plan.push((header, Out::Key(pos)));
            }
            SelectExpr::Aggregate { func, arg } => {
                let resolved = match arg {
                    AggArg::Star => None,
                    AggArg::Column(name) => Some(col_index(table, name)?),
                };
                let header = item.alias.clone().unwrap_or_else(|| aggregate_name(*func, arg));
                plan.push((header, Out::Agg { func: *func, arg: resolved }));
            }
        }
    }

    if plan.is_empty() {
        bail!("the SELECT list is empty");
    }

    let columns: Vec<String> = plan.iter().map(|(name, _)| name.clone()).collect();
    let mut rows = Vec::with_capacity(buckets.len());
    for (key, group) in &buckets {
        let mut out = Vec::with_capacity(plan.len());
        for (_, item) in &plan {
            match item {
                Out::Key(i) => out.push(key.get(*i).cloned().unwrap_or(Value::Null)),
                Out::Agg { func, arg } => out.push(aggregate_value(*func, *arg, table, group)),
            }
        }
        rows.push(out);
    }

    Ok(Built { columns, rows, source: None })
}

fn aggregate_name(func: AggFunc, arg: &AggArg) -> String {
    match arg {
        AggArg::Star => format!("{}(*)", func.name()),
        AggArg::Column(name) => format!("{}({})", func.name(), name),
    }
}

fn aggregate_value(func: AggFunc, arg: Option<usize>, table: &Table, rows: &[usize]) -> Value {
    match func {
        AggFunc::Count => match arg {
            None => Value::Number(rows.len() as f64),
            Some(col) => {
                let n = rows
                    .iter()
                    .filter(|&&r| !table.rows[r][col].is_null())
                    .count();
                Value::Number(n as f64)
            }
        },
        AggFunc::Sum | AggFunc::Avg => {
            let Some(col) = arg else { return Value::Null };
            let mut sum = 0.0;
            let mut count = 0usize;
            for &r in rows {
                if let Some(n) = table.rows[r][col].as_number() {
                    sum += n;
                    count += 1;
                }
            }
            if count == 0 {
                return Value::Null;
            }
            if func == AggFunc::Avg {
                Value::Number(sum / count as f64)
            } else {
                Value::Number(sum)
            }
        }
        AggFunc::Min | AggFunc::Max => {
            let Some(col) = arg else { return Value::Null };
            let mut best: Option<Value> = None;
            for &r in rows {
                let value = &table.rows[r][col];
                if value.is_null() {
                    continue;
                }
                let take = match &best {
                    None => true,
                    Some(current) => {
                        let ord = value.compare(current);
                        if func == AggFunc::Min {
                            ord == Ordering::Less
                        } else {
                            ord == Ordering::Greater
                        }
                    }
                };
                if take {
                    best = Some(value.clone());
                }
            }
            best.unwrap_or(Value::Null)
        }
    }
}

// ---------------------------------------------------------------------------
// ORDER BY / LIMIT
// ---------------------------------------------------------------------------

enum OrderSource {
    /// Read from the projected output row.
    Output(usize),
    /// Read from the original source row (plain projection only).
    Input(usize),
}

fn apply_order(table: &Table, built: Built, keys: &[OrderKey]) -> Result<Built> {
    if keys.is_empty() {
        return Ok(built);
    }

    let resolvers: Vec<(OrderSource, bool)> = keys
        .iter()
        .map(|key| {
            let source = if let Some(i) = built
                .columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(key.column.trim()))
            {
                OrderSource::Output(i)
            } else if built.source.is_some() {
                OrderSource::Input(col_index(table, &key.column)?)
            } else {
                bail!(
                    "ORDER BY `{}` is not one of the result columns: {}",
                    key.column,
                    built.columns.join(", ")
                );
            };
            Ok((source, key.desc))
        })
        .collect::<Result<_>>()?;

    let Built { columns, mut rows, source } = built;
    let value_at =
        |row: &Vec<Value>, resolver: &OrderSource, source: &Option<Vec<usize>>, src: usize| -> Value {
            match resolver {
                OrderSource::Output(i) => row[*i].clone(),
                OrderSource::Input(i) => table.rows[source.as_ref().unwrap()[src]][*i].clone(),
            }
        };

    // Pair each row with its source index so `OrderSource::Input` stays valid
    // while sorting.
    let mut indexed: Vec<(usize, Vec<Value>)> = rows.drain(..).enumerate().collect();
    indexed.sort_by(|(sa, a), (sb, b)| {
        for (resolver, desc) in &resolvers {
            let va = value_at(a, resolver, &source, *sa);
            let vb = value_at(b, resolver, &source, *sb);
            let ord = va.compare(&vb);
            let ord = if *desc { ord.reverse() } else { ord };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    rows = indexed.into_iter().map(|(_, row)| row).collect();

    Ok(Built { columns, rows, source })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn col_index(table: &Table, name: &str) -> Result<usize> {
    table.column_index(name).with_context(|| {
        format!(
            "unknown column `{name}`; available columns: {}",
            table.columns.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql;

    fn s(v: &str) -> Value {
        Value::Text(v.to_string())
    }

    fn n(v: f64) -> Value {
        Value::Number(v)
    }

    fn sample() -> Table {
        let columns = ["Region", "Product", "Rep", "Amount"]
            .iter()
            .map(|c| c.to_string())
            .collect();
        let rows = vec![
            vec![s("East"), s("Widget"), s("Alice"), n(100.0)],
            vec![s("East"), s("Widget"), s("Bob"), n(250.0)],
            vec![s("East"), s("Gadget"), s("Alice"), n(300.0)],
            vec![s("West"), s("Widget"), s("Carol"), n(400.0)],
            vec![s("West"), s("Gadget"), s("Carol"), n(150.0)],
            vec![s("North"), s("Widget"), s("Erin"), n(75.0)],
        ];
        Table { columns, rows }
    }

    fn run(query: &str) -> Table {
        let parsed = sql::parse(query).expect("parse");
        execute(&sample(), &parsed).expect("execute")
    }

    #[test]
    fn group_by_sum() {
        let out = run("select Region, Product, SUM(Amount) from data group by Region, Product order by Region, Product");
        assert_eq!(out.columns, vec!["Region", "Product", "SUM(Amount)"]);
        assert_eq!(
            out.rows,
            vec![
                vec![s("East"), s("Gadget"), n(300.0)],
                vec![s("East"), s("Widget"), n(350.0)],
                vec![s("North"), s("Widget"), n(75.0)],
                vec![s("West"), s("Gadget"), n(150.0)],
                vec![s("West"), s("Widget"), n(400.0)],
            ]
        );
    }

    #[test]
    fn spreadsheet_letters_resolve_to_columns() {
        let out = run("select A, SUM(D) from data group by A order by A");
        assert_eq!(out.rows, vec![
            vec![s("East"), n(650.0)],
            vec![s("North"), n(75.0)],
            vec![s("West"), n(550.0)],
        ]);
    }

    #[test]
    fn where_and_aggregate_without_group_by() {
        let out = run("select COUNT(*) as n, SUM(Amount) as total from data where Amount > 100");
        assert_eq!(out.columns, vec!["n", "total"]);
        assert_eq!(out.rows, vec![vec![n(4.0), n(1100.0)]]);
    }

    #[test]
    fn order_by_alias_desc_with_limit() {
        let out = run("select Region, SUM(Amount) as total from data group by Region order by total desc limit 2");
        assert_eq!(out.rows, vec![vec![s("East"), n(650.0)], vec![s("West"), n(550.0)]]);
    }

    #[test]
    fn min_max_over_text() {
        let out = run("select MIN(Rep) as lo, MAX(Rep) as hi from data");
        assert_eq!(out.rows, vec![vec![s("Alice"), s("Erin")]]);
    }

    #[test]
    fn avg_skips_nulls() {
        let mut table = sample();
        table.rows[0][3] = Value::Null;
        let parsed = sql::parse("select AVG(Amount) as a, COUNT(Amount) as c from data").unwrap();
        let out = execute(&table, &parsed).unwrap();
        // (250+300+400+150+75) / 5
        assert_eq!(out.rows, vec![vec![n(235.0), n(5.0)]]);
    }

    #[test]
    fn unknown_column_is_an_error() {
        let parsed = sql::parse("select Nope from data").unwrap();
        let err = execute(&sample(), &parsed).unwrap_err().to_string();
        assert!(err.contains("unknown column `Nope`"), "{err}");
    }

    #[test]
    fn ungrouped_column_is_rejected() {
        let parsed = sql::parse("select Region, Rep, SUM(Amount) from data group by Region").unwrap();
        let err = execute(&sample(), &parsed).unwrap_err().to_string();
        assert!(err.contains("must appear in GROUP BY"), "{err}");
    }
}

