//! Rendering a result [`Table`] as an aligned text table or CSV.

use crate::table::Table;
use crate::value::Value;

/// Render an ASCII table with a header separator. Numeric columns are
/// right-aligned, everything else is left-aligned.
pub fn to_text(table: &Table) -> String {
    if table.columns.is_empty() {
        return String::new();
    }

    let mut cells: Vec<Vec<String>> = Vec::with_capacity(table.rows.len() + 1);
    cells.push(table.columns.iter().map(|c| c.to_string()).collect());
    for row in &table.rows {
        cells.push(row.iter().map(|v| v.to_string()).collect());
    }

    let widths: Vec<usize> = (0..table.columns.len())
        .map(|c| cells.iter().map(|row| row[c].chars().count()).max().unwrap_or(0))
        .collect();

    let numeric: Vec<bool> = (0..table.columns.len())
        .map(|c| {
            let mut any = false;
            for row in &table.rows {
                match row[c] {
                    Value::Number(_) => any = true,
                    Value::Null => {}
                    _ => return false,
                }
            }
            any
        })
        .collect();

    let mut out = String::new();
    for (r, row) in cells.iter().enumerate() {
        let mut line = String::new();
        for (c, cell) in row.iter().enumerate() {
            let pad = widths[c].saturating_sub(cell.chars().count());
            if c > 0 {
                line.push_str("  ");
            }
            if r > 0 && numeric[c] {
                line.push_str(&" ".repeat(pad));
                line.push_str(cell);
            } else {
                line.push_str(cell);
                line.push_str(&" ".repeat(pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');

        if r == 0 {
            let mut sep = String::new();
            for (c, width) in widths.iter().enumerate() {
                if c > 0 {
                    sep.push_str("  ");
                }
                sep.push_str(&"-".repeat(*width));
            }
            out.push_str(&sep);
            out.push('\n');
        }
    }

    out
}

/// Render the result as RFC-4180-style CSV.
pub fn to_csv(table: &Table) -> String {
    let mut out = String::new();
    out.push_str(&csv_row(table.columns.iter().map(|s| s.as_str())));
    for row in &table.rows {
        let fields: Vec<String> = row.iter().map(|v| v.to_string()).collect();
        out.push_str(&csv_row(fields.iter().map(|s| s.as_str())));
    }
    out
}

fn csv_row<'a>(fields: impl Iterator<Item = &'a str>) -> String {
    let mut line = String::new();
    for (i, field) in fields.enumerate() {
        if i > 0 {
            line.push(',');
        }
        if field.contains([',', '"', '\n', '\r']) {
            line.push('"');
            line.push_str(&field.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(field);
        }
    }
    line.push('\n');
    line
}
