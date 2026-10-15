//! Spreadsheets (XLSX/XLSM/XLSB/XLS/ODS via calamine, CSV/TSV via csv) as compact markdown tables:
//! per sheet the header and the first rows, bounded columns and cell length, plus true row/column counts.

use std::io::Cursor;

use calamine::{Data, Reader, SheetVisible, open_workbook_auto_from_rs};
use serde_json::{Value, json};

use super::{Parsed, extension};
use crate::mail::sources::normalize::truncate_chars;

/// Data rows shown per sheet (after the header).
pub const MAX_ROWS: usize = 200;
pub const MAX_COLUMNS: usize = 50;
pub const MAX_CELL_CHARS: usize = 200;
const MAX_SHEETS: usize = 20;
/// Rows and columns of the small sample kept in the metadata for the UI.
const SAMPLE_ROWS: usize = 6;
const SAMPLE_COLUMNS: usize = 8;
const SAMPLE_CELL_CHARS: usize = 40;

struct Table {
    name: String,
    /// Header plus up to `MAX_ROWS` data rows, each at most `MAX_COLUMNS` cells.
    rows: Vec<Vec<String>>,
    /// Non-empty rows after the header, and the widest row, before any cap.
    data_rows: usize,
    columns: usize,
}

pub fn read(filename: &str, media_type: Option<&str>, bytes: &[u8]) -> Result<Parsed, String> {
    let mt = media_type.unwrap_or_default().to_ascii_lowercase();
    let ext = extension(filename);
    let delimited = matches!(ext.as_str(), "csv" | "tsv") || mt == "text/csv" || mt.contains("tab-separated");
    let tables = if delimited { vec![read_delimited(filename, &ext, bytes)?] } else { read_workbook(bytes)? };
    if tables.is_empty() {
        return Err("The spreadsheet has no visible sheets".into());
    }
    Ok(render(&tables))
}

fn cell_text(raw: &str) -> String {
    let flat: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    truncate_chars(flat.trim(), MAX_CELL_CHARS)
}

fn data_cell(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => cell_text(s),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => format!("{f}"),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => {
            dt.as_datetime().map(|t| t.to_string().trim_end_matches(" 00:00:00").to_string()).unwrap_or_else(|| dt.as_f64().to_string())
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => cell_text(s),
        Data::Error(e) => format!("#{e}"),
    }
}

fn read_workbook(bytes: &[u8]) -> Result<Vec<Table>, String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| format!("Not a readable spreadsheet: {e}"))?;
    let visible: Vec<String> = workbook
        .sheets_metadata()
        .iter()
        .filter(|s| s.visible == SheetVisible::Visible)
        .map(|s| s.name.clone())
        .take(MAX_SHEETS)
        .collect();
    let mut tables = Vec::new();
    for name in visible {
        let range = workbook.worksheet_range(&name).map_err(|e| format!("Could not read sheet: {e}"))?;
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut non_empty = 0usize;
        let mut columns = 0usize;
        for row in range.rows() {
            if row.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }
            non_empty += 1;
            columns = columns.max(row.iter().rposition(|c| !matches!(c, Data::Empty)).map_or(0, |i| i + 1));
            if rows.len() <= MAX_ROWS {
                rows.push(row.iter().take(MAX_COLUMNS).map(data_cell).collect());
            }
        }
        if non_empty > 0 {
            tables.push(Table { name, rows, data_rows: non_empty - 1, columns });
        }
    }
    Ok(tables)
}

fn read_delimited(filename: &str, ext: &str, bytes: &[u8]) -> Result<Table, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{FEFF}');
    let delimiter = if ext == "tsv" {
        b'\t'
    } else {
        let first = text.lines().next().unwrap_or_default();
        b",;\t|".iter().copied().max_by_key(|d| first.bytes().filter(|b| b == d).count()).unwrap_or(b',')
    };
    let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delimiter).from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut non_empty = 0usize;
    let mut columns = 0usize;
    for record in reader.records() {
        let record = record.map_err(|e| format!("Not a readable CSV: {e}"))?;
        if record.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        non_empty += 1;
        columns = columns.max(record.len());
        if rows.len() <= MAX_ROWS {
            rows.push(record.iter().take(MAX_COLUMNS).map(cell_text).collect());
        }
    }
    let name = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem).to_string();
    Ok(Table { name, rows, data_rows: non_empty.saturating_sub(1), columns })
}

fn markdown_cell(c: &str) -> String {
    c.replace('|', "\\|")
}

fn markdown_row(row: &[String], width: usize) -> String {
    let cells: Vec<String> = (0..width).map(|i| markdown_cell(row.get(i).map_or("", String::as_str))).collect();
    format!("| {} |", cells.join(" | "))
}

fn render(tables: &[Table]) -> Parsed {
    let mut text = String::new();
    let mut sheets = Vec::new();
    let mut summary = Vec::new();
    for t in tables {
        let shown_columns = t.columns.clamp(1, MAX_COLUMNS);
        text.push_str(&format!("## Sheet: {} ({} rows × {} columns)\n", t.name, t.data_rows, t.columns));
        if let Some((header, rest)) = t.rows.split_first() {
            text.push_str(&markdown_row(header, shown_columns));
            text.push('\n');
            text.push_str(&format!("|{}\n", " --- |".repeat(shown_columns)));
            for row in rest {
                text.push_str(&markdown_row(row, shown_columns));
                text.push('\n');
            }
        }
        let rows_cut = t.data_rows > MAX_ROWS;
        let columns_cut = t.columns > MAX_COLUMNS;
        if rows_cut || columns_cut {
            text.push_str(&format!(
                "_Showing the first {} of {} rows and {} of {} columns._\n",
                t.data_rows.min(MAX_ROWS),
                t.data_rows,
                shown_columns,
                t.columns
            ));
        }
        text.push('\n');
        let sample: Vec<Vec<String>> = t
            .rows
            .iter()
            .take(SAMPLE_ROWS)
            .map(|r| r.iter().take(SAMPLE_COLUMNS).map(|c| truncate_chars(c, SAMPLE_CELL_CHARS)).collect())
            .collect();
        sheets.push(json!({
            "name": t.name, "rows": t.data_rows, "columns": t.columns,
            "truncated": rows_cut || columns_cut, "sample": sample,
        }));
        summary.push(format!("{}: {} rows × {} columns", t.name, t.data_rows, t.columns));
    }
    Parsed { text, meta: json!({ "summary": truncate_chars(&summary.join("; "), 300), "sheets": Value::Array(sheets) }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_becomes_a_table_with_counts_and_caps() {
        let mut csv = String::from("name,city,notes\n");
        for i in 0..250 {
            csv.push_str(&format!("p{i},Pune,\"a|b\"\n"));
        }
        csv.push_str(&format!("long,x,{}\n", "z".repeat(500)));
        let p = read("people.csv", Some("text/csv"), csv.as_bytes()).unwrap();
        assert!(p.text.starts_with("## Sheet: people (251 rows × 3 columns)\n| name | city | notes |\n| --- | --- | --- |\n| p0 | Pune | a\\|b |"));
        assert!(p.text.contains("| p199 |") && !p.text.contains("| p200 |"));
        assert!(p.text.contains("_Showing the first 200 of 251 rows and 3 of 3 columns._"));
        assert_eq!(p.meta["sheets"][0]["rows"], 251);
        assert_eq!(p.meta["sheets"][0]["sample"].as_array().unwrap().len(), SAMPLE_ROWS);
    }

    #[test]
    fn long_cells_and_wide_sheets_are_capped() {
        let header: Vec<String> = (0..60).map(|i| format!("c{i}")).collect();
        let row: Vec<String> = (0..60).map(|_| "q".repeat(300)).collect();
        let csv = format!("{}\n{}\n", header.join(","), row.join(","));
        let p = read("wide.csv", None, csv.as_bytes()).unwrap();
        assert!(p.text.contains("(1 rows × 60 columns)"));
        assert!(p.text.contains("| c49 |") && !p.text.contains("c50"));
        assert!(!p.text.contains(&"q".repeat(201)));
        assert!(p.text.contains("50 of 60 columns"));
    }

    #[test]
    fn semicolon_and_tab_delimiters_are_detected() {
        let semi = read("a.csv", None, b"x;y\n1;2\n").unwrap();
        assert!(semi.text.contains("| x | y |") && semi.text.contains("| 1 | 2 |"));
        let tab = read("a.tsv", Some("text/tab-separated-values"), b"x\ty\n1\t2\n").unwrap();
        assert!(tab.text.contains("| x | y |"));
    }

    #[test]
    fn xlsx_reads_every_visible_sheet() {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sales = book.add_worksheet();
        sales.set_name("Sales").unwrap();
        sales.write_string(0, 0, "Region").unwrap();
        sales.write_string(0, 1, "Revenue").unwrap();
        sales.write_string(1, 0, "Zanzibar").unwrap();
        sales.write_number(1, 1, 1200.0).unwrap();
        sales.write_number(2, 1, 99.5).unwrap();
        let hidden = book.add_worksheet();
        hidden.set_name("Secret").unwrap();
        hidden.write_string(0, 0, "hidden-data").unwrap();
        hidden.set_hidden(true);
        let bytes = book.save_to_buffer().unwrap();
        let p = read("q3.xlsx", None, &bytes).unwrap();
        assert!(p.text.contains("## Sheet: Sales (2 rows × 2 columns)"), "{}", p.text);
        assert!(p.text.contains("| Zanzibar | 1200 |") && p.text.contains("99.5"));
        assert!(!p.text.contains("hidden-data"));
        assert_eq!(p.meta["sheets"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(read("x.xlsx", None, b"definitely not a zip").is_err());
    }
}
