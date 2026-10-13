# excel-reader

Read a spreadsheet and answer SQL-like questions about it from the command line.
Columns can be referenced by their header name **or** by their spreadsheet
letter (`A`, `B`, `C`, ...), so this works out of the box:

```console
$ excel-reader sales.xlsx "select A, B, SUM(D) from data group by A, B"
```

```
A       B       SUM(D)
------  ------  ------
East    Gadget     300
East    Widget     350
North   Widget      75
West    Gadget     300
West    Widget     400
```

## Install / build

```console
cargo build --release
# binary at target/release/excel-reader(.exe)
```

## Usage

```text
excel-reader [OPTIONS] <FILE> <QUERY>

Arguments:
  <FILE>   Path to the workbook (.xlsx, .xlsm, .xls, .xlsb, .ods)
  <QUERY>  The query to run

Options:
  -s, --sheet <SHEET>   Sheet to read (defaults to the first sheet)
      --no-header       Treat every row as data; name columns A, B, C, ...
  -f, --format <FORMAT> table | csv  [default: table]
      --sheets          List the sheet names and exit
  -h, --help            Print help
  -V, --version         Print version
```

The `FROM` clause takes an arbitrary table name (conventionally `data`); it is
not matched against the file or sheet name.

## Supported query features

- `SELECT`: bare columns, `*`, aliases (`AS name` or a trailing name), and the
  aggregates `SUM`, `COUNT`, `AVG`/`AVERAGE`, `MIN`, `MAX` (also `COUNT(*)`).
- `WHERE`: `=`, `<>`/`!=`, `<`, `<=`, `>`, `>=`, `IS NULL`, `IS NOT NULL`,
  combined with `AND`, `OR`, `NOT` and parentheses.
- `GROUP BY`: one or more columns.
- `ORDER BY`: result columns or aliases, with `ASC`/`DESC`.
- `LIMIT`: a row count.

Semantics follow SQL conventions: aggregating functions ignore `NULL`s, `SUM`
of no rows is `NULL`, and any comparison against `NULL` is false. Combining an
aggregate with an ungrouped bare column is an error. `HAVING` is intentionally
not supported — use `WHERE` to filter source rows.

## Column resolution

A column reference resolves in this order:

1. header name, case-insensitively (`Amount`, `amount`)
2. spreadsheet column letter, case-insensitively (`A`, `B`, `AA`)
3. synthetic `col_1`, `col_2`, ... (1-based position)

Quoted identifiers are also accepted: `"Amount"`, `` `Amount` ``, `[Amount]`,
and string literals use single quotes with `''` as an escaped quote — handy for
headers that collide with keywords or contain spaces.

## Examples

```console
# total amount per region, largest first
excel-reader sales.xlsx \
  "select Region, SUM(Amount) as total from data group by Region order by total desc"

# filter before aggregating, CSV output
excel-reader sales.xlsx \
  "select Rep, COUNT(*) as orders, AVG(Amount) as avg from data where Amount >= 100 group by Rep" \
  --format csv

# read a sheet whose columns have no headers
excel-reader report.xlsx "select A, SUM(C) from data group by A" --no-header --sheet Q3

# discover sheet names
excel-reader sales.xlsx --sheets
```

## Development

```console
cargo test        # unit tests for the parser and engine
cargo clippy      # lints

python samples/generate.py   # regenerate samples/sales.xlsx (needs openpyxl)
```

## Layout

| File               | Responsibility                                   |
| ------------------ | ------------------------------------------------ |
| `src/main.rs`      | CLI parsing and wiring                           |
| `src/sql.rs`       | Lexer, AST, and recursive-descent parser         |
| `src/engine.rs`    | Filtering, grouping, aggregation, ordering       |
| `src/table.rs`     | Loading worksheets into an in-memory table       |
| `src/value.rs`     | Cell value type and ordering/comparison rules    |
| `src/render.rs`    | Aligned text and CSV rendering                   |
