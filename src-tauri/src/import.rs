//! Reading CSV and Excel files into rows, and suggesting how columns map onto
//! contact fields. Writing to the database lives in `contacts.rs`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::templates::normalise_key;

#[derive(Debug, Clone, Serialize)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Where one column goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Target {
    Email,
    FirstName,
    LastName,
    /// Custom field, stored under this (normalised) key.
    Custom(String),
    Ignore,
}

pub fn read_table(path: &Path) -> AppResult<Table> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let table = match ext.as_str() {
        "csv" | "txt" | "tsv" => read_csv(path)?,
        "xlsx" | "xlsm" | "xls" | "ods" => read_sheet(path)?,
        _ => return Err(AppError::user("Choose a .csv or .xlsx file.")),
    };
    if table.headers.iter().all(|h| h.trim().is_empty()) {
        return Err(AppError::user(
            "The first row of the file should contain column names (like \"email\" and \"first name\").",
        ));
    }
    Ok(table)
}

fn read_csv(path: &Path) -> AppResult<Table> {
    let bytes = std::fs::read(path)?;
    // Excel often saves CSV with a byte-order mark.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes);
    let first_line = text.lines().next().unwrap_or("");
    // European Excel uses ';', some exports use tabs.
    let delimiter = b",;\t"
        .iter()
        .copied()
        .max_by_key(|d| first_line.matches(*d as char).count())
        .unwrap_or(b',');
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut records = reader.records();
    let headers = match records.next() {
        Some(r) => r
            .map_err(|e| AppError::user(format!("Couldn't read the CSV file: {e}")))?
            .iter()
            .map(|s| s.trim().to_string())
            .collect(),
        None => return Err(AppError::user("That file is empty.")),
    };
    let mut rows = Vec::new();
    for r in records {
        let r = r.map_err(|e| AppError::user(format!("Couldn't read the CSV file: {e}")))?;
        let row: Vec<String> = r.iter().map(|s| s.trim().to_string()).collect();
        if row.iter().any(|c| !c.is_empty()) {
            rows.push(row);
        }
    }
    Ok(Table { headers, rows })
}

fn read_sheet(path: &Path) -> AppResult<Table> {
    use calamine::{open_workbook_auto, Data, Reader};
    let mut wb = open_workbook_auto(path)
        .map_err(|e| AppError::user(format!("Couldn't open the spreadsheet: {e}")))?;
    let name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| AppError::user("That spreadsheet has no sheets."))?;
    let range = wb
        .worksheet_range(&name)
        .map_err(|e| AppError::user(format!("Couldn't read the spreadsheet: {e}")))?;
    let cell = |c: &Data| -> String {
        match c {
            Data::Empty => String::new(),
            Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
            other => other.to_string().trim().to_string(),
        }
    };
    let mut iter = range.rows();
    let headers = iter
        .next()
        .ok_or_else(|| AppError::user("That spreadsheet is empty."))?
        .iter()
        .map(cell)
        .collect();
    let rows = iter
        .map(|r| r.iter().map(cell).collect::<Vec<_>>())
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .collect();
    Ok(Table { headers, rows })
}

/// Match columns by header name. The user confirms or changes this.
pub fn suggest_mapping(headers: &[String]) -> Vec<Target> {
    let mut have_email = false;
    headers
        .iter()
        .map(|h| {
            let key = normalise_key(h);
            let compact = key.replace('_', "");
            let t = match compact.as_str() {
                "email" | "emailaddress" | "mail" | "e_mail" | "emails" | "workemail" if !have_email => {
                    have_email = true;
                    Target::Email
                }
                "firstname" | "first" | "givenname" | "forename" | "fname" => Target::FirstName,
                "lastname" | "last" | "surname" | "familyname" | "lname" => Target::LastName,
                "" => Target::Ignore,
                _ => Target::Custom(key),
            };
            t
        })
        .collect()
}

/// Lowercase and trim. Addresses are de-duplicated on this form.
pub fn normalise_email(s: &str) -> String {
    s.trim().trim_matches(|c| c == '<' || c == '>').trim().to_ascii_lowercase()
}

pub fn is_valid_email(s: &str) -> bool {
    use std::str::FromStr;
    let s = s.trim();
    // email_address accepts things like "a@b" — require a dot in the domain.
    match email_address::EmailAddress::from_str(s) {
        Ok(addr) => {
            let domain = addr.domain();
            domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !s.contains(' ')
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_with_bom_and_semicolons() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.csv");
        std::fs::write(&p, "\u{FEFF}Email;First Name;Company\nA@x.com ; Ana;Acme\n\n;;\nb@y.com;Bob;\n").unwrap();
        let t = read_table(&p).unwrap();
        assert_eq!(t.headers, vec!["Email", "First Name", "Company"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0], vec!["A@x.com", "Ana", "Acme"]);
    }

    #[test]
    fn csv_quoted_commas() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.csv");
        std::fs::write(&p, "email,company\n\"a@x.com\",\"Acme, Inc.\"\n").unwrap();
        let t = read_table(&p).unwrap();
        assert_eq!(t.rows[0][1], "Acme, Inc.");
    }

    #[test]
    fn xlsx_reads_first_sheet() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.xlsx");
        let mut wb = rust_xlsxwriter::Workbook::new();
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "E-mail").unwrap();
        ws.write_string(0, 1, "Surname").unwrap();
        ws.write_string(0, 2, "Employees").unwrap();
        ws.write_string(1, 0, "a@x.com").unwrap();
        ws.write_string(1, 1, "Lopez").unwrap();
        ws.write_number(1, 2, 250.0).unwrap();
        wb.save(&p).unwrap();
        let t = read_table(&p).unwrap();
        assert_eq!(t.headers, vec!["E-mail", "Surname", "Employees"]);
        assert_eq!(t.rows, vec![vec!["a@x.com", "Lopez", "250"]]);
        assert_eq!(
            suggest_mapping(&t.headers),
            vec![Target::Email, Target::LastName, Target::Custom("employees".into())]
        );
    }

    #[test]
    fn mapping_by_header_name() {
        let h: Vec<String> = ["Email Address", "first_name", "Last Name", "Company Name", "", "email"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            suggest_mapping(&h),
            vec![
                Target::Email,
                Target::FirstName,
                Target::LastName,
                Target::Custom("company_name".into()),
                Target::Ignore,
                Target::Custom("email".into()),
            ]
        );
    }

    #[test]
    fn email_validation() {
        for ok in ["a@x.com", "first.last+tag@sub.example.co.uk"] {
            assert!(is_valid_email(ok), "{ok}");
        }
        for bad in ["", "a@", "@x.com", "a@x", "a b@x.com", "not an email", "a@x..com"] {
            assert!(!is_valid_email(bad), "{bad}");
        }
        assert_eq!(normalise_email("  <Ana@X.COM> "), "ana@x.com");
    }

    #[test]
    fn rejects_unknown_extension() {
        assert!(read_table(Path::new("contacts.pdf")).is_err());
    }
}
