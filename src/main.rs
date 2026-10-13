//! excel-reader: run SQL-like queries against a spreadsheet.
//!
//! ```text
//! excel-reader data.xlsx "select A,B,C,SUM(D) from data group by A,B,C"
//! ```

mod engine;
mod render;
mod sql;
mod table;
mod value;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use calamine::Reader;

use crate::table::Table;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    /// Aligned text table.
    Table,
    /// RFC-4180 CSV.
    Csv,
}

#[derive(Debug, Parser)]
#[command(
    name = "excel-reader",
    version,
    about = "Query Excel/ODS workbooks with a small SQL dialect",
    after_help = "Example:\n  excel-reader sales.xlsx \"select Region, SUM(Amount) as total from data group by Region order by total desc\""
)]
struct Cli {
    /// Path to the workbook (.xlsx, .xlsm, .xls, .xlsb, .ods).
    file: Option<String>,

    /// The query to run, e.g. "select A,B,C,SUM(D) from data group by A,B,C".
    query: Option<String>,

    /// Sheet to read (defaults to the first sheet).
    #[arg(short, long)]
    sheet: Option<String>,

    /// Treat every row as data and name the columns A, B, C, ... instead of
    /// using the first row as the header.
    #[arg(long)]
    no_header: bool,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Table)]
    format: Format,

    /// List the sheet names in the workbook and exit.
    #[arg(long)]
    sheets: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let file = cli
        .file
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("a workbook path is required (see --help)"))?;

    if cli.sheets {
        for name in sheet_names(file)? {
            println!("{name}");
        }
        return Ok(());
    }

    let table = Table::from_workbook(file, cli.sheet.as_deref(), !cli.no_header)?;

    let query = cli
        .query
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("a query is required (see --help)"))?;
    let parsed = sql::parse(query)?;
    let result = engine::execute(&table, &parsed)?;

    let rendered = match cli.format {
        Format::Table => render::to_text(&result),
        Format::Csv => render::to_csv(&result),
    };
    print!("{rendered}");
    Ok(())
}

fn sheet_names(file: &str) -> Result<Vec<String>> {
    let workbook = calamine::open_workbook_auto(file)?;
    Ok(workbook.sheet_names())
}
