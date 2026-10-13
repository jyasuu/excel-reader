//! Reading worksheets into an in-memory [`Table`].

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader, open_workbook_auto};

use crate::value::Value;

/// A rectangular, in-memory table: a header row plus data rows.
#[derive(Debug, Clone)]
pub struct Table {
    /// Display names of the columns (from the header row).
    pub columns: Vec<String>,
    /// Data rows. Every row has `columns.len()` cells.
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Resolve a column reference to an index.
    ///
    /// Resolution order:
    /// 1. exact header name (case-insensitive)
    /// 2. the spreadsheet column letter, e.g. `A`, `B`, `AA` (case-insensitive)
    /// 3. the synthetic name `col_1`, `col_2`, ... (1-based)
    pub fn column_index(&self, name: &str) -> Option<usize> {
        let needle = name.trim();

        if let Some(i) = self
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(needle))
        {
            return Some(i);
        }

        if let Some(i) = (0..self.columns.len())
            .find(|&i| index_to_column_letter(i).eq_ignore_ascii_case(needle))
        {
            return Some(i);
        }

        if let Some(rest) = needle.to_ascii_lowercase().strip_prefix("col_")
            && let Ok(n) = rest.parse::<usize>()
            && n >= 1
            && n <= self.columns.len()
        {
            return Some(n - 1);
        }

        None
    }

    /// Load a workbook sheet into a table. When `sheet` is `None` the first
    /// sheet is used. When `has_header` is true the first non-empty row becomes
    /// the header, otherwise columns are named `A`, `B`, `C`, ...
    pub fn from_workbook(path: &str, sheet: Option<&str>, has_header: bool) -> Result<Table> {
        let mut workbook =
            open_workbook_auto(path).with_context(|| format!("failed to open workbook '{path}'"))?;

        let names = workbook.sheet_names();
        if names.is_empty() {
            bail!("workbook '{path}' contains no sheets");
        }

        let sheet_name = match sheet {
            Some(want) => names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(want))
                .cloned()
                .with_context(|| {
                    format!("sheet '{want}' not found; available sheets: {}", names.join(", "))
                })?,
            None => names[0].clone(),
        };

        let range = workbook
            .worksheet_range(&sheet_name)
            .with_context(|| format!("failed to read sheet '{sheet_name}'"))?;

        Table::from_range(range, has_header)
    }

    fn from_range(range: calamine::Range<Data>, has_header: bool) -> Result<Table> {
        let mut raw: Vec<Vec<Value>> = range
            .rows()
            .map(|row| row.iter().map(data_to_value).collect())
            .collect();

        // Drop fully empty leading rows so the real data starts here.
        while raw.first().is_some_and(|row| row.iter().all(Value::is_null)) {
            raw.remove(0);
        }

        if raw.is_empty() {
            bail!("the selected sheet is empty");
        }

        let columns: Vec<String> = if has_header {
            let header = raw.remove(0);
            header
                .iter()
                .enumerate()
                .map(|(i, v)| match v {
                    Value::Null => index_to_column_letter(i),
                    other => other.to_string(),
                })
                .collect()
        } else {
            let width = raw.iter().map(Vec::len).max().unwrap_or(0);
            (0..width).map(index_to_column_letter).collect()
        };

        // Trim trailing columns that contain no data at all.
        let last_used = (0..columns.len())
            .rev()
            .find(|&c| raw.iter().any(|row| !row[c].is_null()))
            .map(|c| c + 1)
            .unwrap_or(0);

        let columns = columns[..last_used].to_vec();

        let rows: Vec<Vec<Value>> = raw
            .into_iter()
            .filter(|row| row.iter().any(|v| !v.is_null()))
            .map(|mut row| {
                row.resize(columns.len(), Value::Null);
                row.truncate(columns.len());
                row
            })
            .collect();

        Ok(Table { columns, rows })
    }
}

fn data_to_value(data: &Data) -> Value {
    match data {
        Data::Int(i) => Value::Number(*i as f64),
        Data::Float(f) => Value::Number(*f),
        Data::String(s) => Value::Text(s.clone()),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::Text(format_excel_datetime(dt)),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::Text(s.clone()),
        Data::Error(e) => Value::Text(format!("{e:?}")),
        Data::Empty => Value::Null,
    }
}

fn format_excel_datetime(dt: &calamine::ExcelDateTime) -> String {
    if dt.is_duration() {
        return format!("{}", dt.as_f64());
    }
    let (y, mo, d, h, mi, s, _ms) = dt.to_ymd_hms_milli();
    if h == 0 && mi == 0 && s == 0 {
        format!("{y:04}-{mo:02}-{d:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
    }
}

/// Spreadsheet column letter for a zero-based index: 0 -> A, 25 -> Z, 26 -> AA.
pub fn index_to_column_letter(mut idx: usize) -> String {
    let mut out = String::new();
    loop {
        out.insert(0, (b'A' + (idx % 26) as u8) as char);
        if idx < 26 {
            break;
        }
        idx = idx / 26 - 1;
    }
    out
}
