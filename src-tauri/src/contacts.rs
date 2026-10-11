//! Contacts, lists, tags and the global do-not-contact list.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::clock::ts;
use crate::error::{AppError, AppResult};
use crate::import::{is_valid_email, normalise_email, Table, Target};
use crate::templates::normalise_key;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    #[serde(default)]
    pub id: i64,
    pub email: String,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub lists: Vec<i64>,
    #[serde(default)]
    pub do_not_contact: bool,
    #[serde(default)]
    pub created_at: String,
}

impl Contact {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        let fields: String = r.get("fields")?;
        Ok(Contact {
            id: r.get("id")?,
            email: r.get("email")?,
            first_name: r.get("first_name")?,
            last_name: r.get("last_name")?,
            fields: serde_json::from_str(&fields).unwrap_or_default(),
            tags: Vec::new(),
            lists: Vec::new(),
            do_not_contact: r.get::<_, Option<i64>>("dnc").ok().flatten().unwrap_or(0) != 0,
            created_at: r.get("created_at")?,
        })
    }

    /// All merge fields for this contact, keys normalised.
    pub fn merge_fields(&self) -> HashMap<String, String> {
        let mut m: HashMap<String, String> = self
            .fields
            .iter()
            .map(|(k, v)| (normalise_key(k), v.clone()))
            .collect();
        m.insert("email".into(), self.email.clone());
        m.insert("first_name".into(), self.first_name.clone());
        m.insert("last_name".into(), self.last_name.clone());
        m
    }

    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name).trim().to_string()
    }
}

const SELECT: &str = "SELECT c.*, (SELECT 1 FROM do_not_contact d WHERE d.email = c.email) AS dnc FROM contacts c";

#[derive(Debug, Default, Deserialize)]
pub struct ContactQuery {
    #[serde(default)]
    pub search: String,
    pub list_id: Option<i64>,
    pub tag: Option<String>,
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_page")]
    pub limit: i64,
}

fn default_page() -> i64 {
    200
}

#[derive(Debug, Serialize)]
pub struct ContactPage {
    pub total: i64,
    pub rows: Vec<Contact>,
}

pub fn query(conn: &Connection, q: &ContactQuery) -> AppResult<ContactPage> {
    let mut where_ = vec!["1 = 1".to_string()];
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if !q.search.trim().is_empty() {
        let like = format!("%{}%", q.search.trim().to_lowercase());
        where_.push("(lower(c.email) LIKE ? OR lower(c.first_name) LIKE ? OR lower(c.last_name) LIKE ? OR lower(c.fields) LIKE ?)".into());
        for _ in 0..4 {
            args.push(like.clone().into());
        }
    }
    if let Some(list) = q.list_id {
        where_.push("c.id IN (SELECT contact_id FROM list_members WHERE list_id = ?)".into());
        args.push(list.into());
    }
    if let Some(tag) = q.tag.as_ref().filter(|t| !t.is_empty()) {
        where_.push("c.id IN (SELECT ct.contact_id FROM contact_tags ct JOIN tags t ON t.id = ct.tag_id WHERE t.name = ?)".into());
        args.push(tag.clone().into());
    }
    let where_sql = where_.join(" AND ");
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM contacts c WHERE {where_sql}"),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    let sql = format!("{SELECT} WHERE {where_sql} ORDER BY c.email LIMIT {} OFFSET {}", q.limit.clamp(1, 5000), q.offset.max(0));
    let mut stmt = conn.prepare(&sql)?;
    let mut rows: Vec<Contact> = stmt
        .query_map(params_from_iter(args.iter()), Contact::from_row)?
        .collect::<Result<_, _>>()?;
    attach_tags_and_lists(conn, &mut rows)?;
    Ok(ContactPage { total, rows })
}

fn attach_tags_and_lists(conn: &Connection, rows: &mut [Contact]) -> AppResult<()> {
    let mut tag_stmt = conn.prepare(
        "SELECT t.name FROM contact_tags ct JOIN tags t ON t.id = ct.tag_id WHERE ct.contact_id = ?1 ORDER BY t.name",
    )?;
    let mut list_stmt = conn.prepare("SELECT list_id FROM list_members WHERE contact_id = ?1")?;
    for c in rows.iter_mut() {
        c.tags = tag_stmt
            .query_map([c.id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        c.lists = list_stmt
            .query_map([c.id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
    }
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Contact> {
    let mut c = conn
        .query_row(&format!("{SELECT} WHERE c.id = ?1"), [id], Contact::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound("contact".into()))?;
    attach_tags_and_lists(conn, std::slice::from_mut(&mut c))?;
    Ok(c)
}

pub fn find_by_email(conn: &Connection, email: &str) -> AppResult<Option<Contact>> {
    Ok(conn
        .query_row(
            &format!("{SELECT} WHERE c.email = ?1"),
            [normalise_email(email)],
            Contact::from_row,
        )
        .optional()?)
}

/// Create or edit one contact by hand.
pub fn save(conn: &Connection, c: &Contact, now: DateTime<Utc>) -> AppResult<Contact> {
    let email = normalise_email(&c.email);
    if !is_valid_email(&email) {
        return Err(AppError::user(format!("\"{}\" isn't a valid email address.", c.email)));
    }
    let fields: BTreeMap<String, String> = c
        .fields
        .iter()
        .map(|(k, v)| (normalise_key(k), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty() && !matches!(k.as_str(), "email" | "first_name" | "last_name"))
        .collect();
    let fields_json = serde_json::to_string(&fields)?;
    let clash: Option<i64> = conn
        .query_row("SELECT id FROM contacts WHERE email = ?1", [&email], |r| r.get(0))
        .optional()?;
    let id = if c.id == 0 {
        if clash.is_some() {
            return Err(AppError::user(format!("{email} is already in your contacts.")));
        }
        conn.execute(
            "INSERT INTO contacts (email, first_name, last_name, fields, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![email, c.first_name.trim(), c.last_name.trim(), fields_json, ts(now)],
        )?;
        conn.last_insert_rowid()
    } else {
        if clash.is_some_and(|other| other != c.id) {
            return Err(AppError::user(format!("{email} is already in your contacts.")));
        }
        conn.execute(
            "UPDATE contacts SET email = ?2, first_name = ?3, last_name = ?4, fields = ?5, updated_at = ?6 WHERE id = ?1",
            params![c.id, email, c.first_name.trim(), c.last_name.trim(), fields_json, ts(now)],
        )?;
        c.id
    };
    set_tags(conn, id, &c.tags)?;
    conn.execute("DELETE FROM list_members WHERE contact_id = ?1", [id])?;
    for list in &c.lists {
        conn.execute(
            "INSERT OR IGNORE INTO list_members (list_id, contact_id) VALUES (?1, ?2)",
            params![list, id],
        )?;
    }
    get(conn, id)
}

pub fn delete(conn: &Connection, ids: &[i64]) -> AppResult<usize> {
    let mut n = 0;
    for id in ids {
        n += conn.execute("DELETE FROM contacts WHERE id = ?1", [id])?;
    }
    Ok(n)
}

/// Every custom field key in use, for the template editor.
pub fn field_names(conn: &Connection) -> AppResult<Vec<String>> {
    let mut names: BTreeSet<String> = ["email", "first_name", "last_name"].iter().map(|s| s.to_string()).collect();
    let mut stmt = conn.prepare("SELECT DISTINCT fields FROM contacts WHERE fields != '{}'")?;
    for f in stmt.query_map([], |r| r.get::<_, String>(0))? {
        if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(&f?) {
            names.extend(map.into_keys());
        }
    }
    Ok(names.into_iter().collect())
}

// ---- tags -----------------------------------------------------------------

fn tag_id(conn: &Connection, name: &str) -> AppResult<i64> {
    conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [name])?;
    Ok(conn.query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))?)
}

pub fn set_tags(conn: &Connection, contact_id: i64, tags: &[String]) -> AppResult<()> {
    conn.execute("DELETE FROM contact_tags WHERE contact_id = ?1", [contact_id])?;
    add_tags(conn, contact_id, tags)?;
    conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM contact_tags)", [])?;
    Ok(())
}

fn add_tags(conn: &Connection, contact_id: i64, tags: &[String]) -> AppResult<()> {
    for t in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let id = tag_id(conn, t)?;
        conn.execute(
            "INSERT OR IGNORE INTO contact_tags (contact_id, tag_id) VALUES (?1, ?2)",
            params![contact_id, id],
        )?;
    }
    Ok(())
}

pub fn list_tags(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM tags ORDER BY name")?;
    let r = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(r)
}

// ---- lists ----------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ContactList {
    pub id: i64,
    pub name: String,
    pub member_count: i64,
}

pub fn lists(conn: &Connection) -> AppResult<Vec<ContactList>> {
    let mut stmt = conn.prepare(
        "SELECT l.id, l.name, (SELECT COUNT(*) FROM list_members m WHERE m.list_id = l.id)
         FROM lists l ORDER BY l.name",
    )?;
    let r = stmt
        .query_map([], |r| {
            Ok(ContactList { id: r.get(0)?, name: r.get(1)?, member_count: r.get(2)? })
        })?
        .collect::<Result<_, _>>()?;
    Ok(r)
}

pub fn create_list(conn: &Connection, name: &str, now: DateTime<Utc>) -> AppResult<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::user("Give the list a name."));
    }
    if let Some(id) = conn
        .query_row("SELECT id FROM lists WHERE name = ?1", [name], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    conn.execute("INSERT INTO lists (name, created_at) VALUES (?1, ?2)", params![name, ts(now)])?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_list(conn: &Connection, id: i64, name: &str) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::user("Give the list a name."));
    }
    conn.execute("UPDATE lists SET name = ?2 WHERE id = ?1", params![id, name])
        .map_err(|_| AppError::user("There's already a list with that name."))?;
    Ok(())
}

pub fn delete_list(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM lists WHERE id = ?1", [id])?;
    Ok(())
}

pub fn add_to_list(conn: &Connection, list_id: i64, contact_ids: &[i64]) -> AppResult<()> {
    for c in contact_ids {
        conn.execute(
            "INSERT OR IGNORE INTO list_members (list_id, contact_id) VALUES (?1, ?2)",
            params![list_id, c],
        )?;
    }
    Ok(())
}

pub fn remove_from_list(conn: &Connection, list_id: i64, contact_ids: &[i64]) -> AppResult<()> {
    for c in contact_ids {
        conn.execute(
            "DELETE FROM list_members WHERE list_id = ?1 AND contact_id = ?2",
            params![list_id, c],
        )?;
    }
    Ok(())
}

pub fn list_member_ids(conn: &Connection, list_id: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT contact_id FROM list_members WHERE list_id = ?1 ORDER BY contact_id")?;
    let r = stmt.query_map([list_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(r)
}

// ---- do-not-contact -------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DncEntry {
    pub email: String,
    pub reason: String,
    pub added_at: String,
}

pub fn is_dnc(conn: &Connection, email: &str) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM do_not_contact WHERE email = ?1",
            [normalise_email(email)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Returns true if the address was newly added.
pub fn add_dnc(conn: &Connection, email: &str, reason: &str, now: DateTime<Utc>) -> AppResult<bool> {
    let email = normalise_email(email);
    if email.is_empty() {
        return Ok(false);
    }
    Ok(conn.execute(
        "INSERT OR IGNORE INTO do_not_contact (email, reason, added_at) VALUES (?1, ?2, ?3)",
        params![email, reason, ts(now)],
    )? > 0)
}

pub fn remove_dnc(conn: &Connection, email: &str) -> AppResult<()> {
    conn.execute("DELETE FROM do_not_contact WHERE email = ?1", [normalise_email(email)])?;
    Ok(())
}

pub fn dnc_list(conn: &Connection) -> AppResult<Vec<DncEntry>> {
    let mut stmt = conn.prepare("SELECT email, reason, added_at FROM do_not_contact ORDER BY added_at DESC, email")?;
    let r = stmt
        .query_map([], |r| Ok(DncEntry { email: r.get(0)?, reason: r.get(1)?, added_at: r.get(2)? }))?
        .collect::<Result<_, _>>()?;
    Ok(r)
}

// ---- import ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub mapping: Vec<Target>,
    pub list_id: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Import into the do-not-contact list instead of contacts.
    #[serde(default)]
    pub to_dnc: bool,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct ImportSummary {
    pub added: usize,
    pub updated: usize,
    pub duplicates: usize,
    pub invalid: usize,
    /// (row number in the file, value) for each invalid row, first 100.
    pub invalid_rows: Vec<(usize, String)>,
}

pub fn import(conn: &mut Connection, table: &Table, req: &ImportRequest, now: DateTime<Utc>) -> AppResult<ImportSummary> {
    let email_col = req
        .mapping
        .iter()
        .position(|t| *t == Target::Email)
        .ok_or_else(|| AppError::user("Choose which column holds the email address."))?;
    let tx = conn.transaction()?;
    let mut s = ImportSummary::default();
    let mut seen = HashSet::new();
    for (i, row) in table.rows.iter().enumerate() {
        let line = i + 2; // 1-based, after the header row
        let raw = row.get(email_col).map(|s| s.as_str()).unwrap_or("");
        let email = normalise_email(raw);
        if !is_valid_email(&email) {
            s.invalid += 1;
            if s.invalid_rows.len() < 100 {
                s.invalid_rows.push((line, raw.to_string()));
            }
            continue;
        }
        if !seen.insert(email.clone()) {
            s.duplicates += 1;
            continue;
        }
        if req.to_dnc {
            if add_dnc(&tx, &email, "import", now)? {
                s.added += 1;
            } else {
                s.duplicates += 1;
            }
            continue;
        }
        let mut first = None;
        let mut last = None;
        let mut custom = BTreeMap::new();
        for (col, target) in req.mapping.iter().enumerate() {
            let v = row.get(col).map(|s| s.trim()).unwrap_or("");
            match target {
                Target::FirstName => first = Some(v),
                Target::LastName => last = Some(v),
                Target::Custom(k) => {
                    let k = normalise_key(k);
                    if !k.is_empty() && !matches!(k.as_str(), "email" | "first_name" | "last_name") {
                        custom.insert(k, v.to_string());
                    }
                }
                Target::Email | Target::Ignore => {}
            }
        }
        let existing: Option<(i64, String, String, String)> = tx
            .query_row(
                "SELECT id, first_name, last_name, fields FROM contacts WHERE email = ?1",
                [&email],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let id = match existing {
            Some((id, old_first, old_last, old_fields)) => {
                // Non-empty values from the file win; blanks keep what we had.
                let mut fields: BTreeMap<String, String> = serde_json::from_str(&old_fields).unwrap_or_default();
                for (k, v) in custom {
                    if !v.is_empty() {
                        fields.insert(k, v);
                    }
                }
                let pick = |new: Option<&str>, old: String| new.filter(|v| !v.is_empty()).map(str::to_string).unwrap_or(old);
                tx.execute(
                    "UPDATE contacts SET first_name = ?2, last_name = ?3, fields = ?4, updated_at = ?5 WHERE id = ?1",
                    params![id, pick(first, old_first), pick(last, old_last), serde_json::to_string(&fields)?, ts(now)],
                )?;
                s.updated += 1;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO contacts (email, first_name, last_name, fields, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![email, first.unwrap_or(""), last.unwrap_or(""), serde_json::to_string(&custom)?, ts(now)],
                )?;
                s.added += 1;
                tx.last_insert_rowid()
            }
        };
        if let Some(list) = req.list_id {
            tx.execute(
                "INSERT OR IGNORE INTO list_members (list_id, contact_id) VALUES (?1, ?2)",
                params![list, id],
            )?;
        }
        add_tags(&tx, id, &req.tags)?;
    }
    tx.commit()?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::parse_ts;
    use crate::db::Db;

    fn now() -> DateTime<Utc> {
        parse_ts("2026-10-12T10:00:00Z").unwrap()
    }

    fn table(rows: &[&[&str]]) -> Table {
        Table {
            headers: rows[0].iter().map(|s| s.to_string()).collect(),
            rows: rows[1..].iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect(),
        }
    }

    #[test]
    fn import_dedupes_validates_and_updates() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.lock();
        let list = create_list(&conn, "Leads", now()).unwrap();
        save(&conn, &Contact { id: 0, email: "old@x.com".into(), first_name: "Old".into(), last_name: "".into(),
            fields: BTreeMap::from([("company".into(), "Keep".into())]), tags: vec![], lists: vec![], do_not_contact: false, created_at: String::new() }, now()).unwrap();
        let t = table(&[
            &["Email", "First name", "Company"],
            &["Ana@X.com", "Ana", "Acme"],
            &[" ana@x.com ", "Ana2", "Dup"],
            &["not-an-email", "Bad", ""],
            &["old@x.com", "", "New Co"],
            &["", "Empty", ""],
        ]);
        let req = ImportRequest {
            mapping: crate::import::suggest_mapping(&t.headers),
            list_id: Some(list),
            tags: vec!["Conference".into()],
            to_dnc: false,
        };
        let s = import(&mut conn, &t, &req, now()).unwrap();
        assert_eq!((s.added, s.updated, s.duplicates, s.invalid), (1, 1, 1, 2));
        assert_eq!(s.invalid_rows[0], (4, "not-an-email".to_string()));
        let ana = find_by_email(&conn, "ana@x.com").unwrap().unwrap();
        assert_eq!(ana.first_name, "Ana");
        assert_eq!(ana.fields["company"], "Acme");
        let old = find_by_email(&conn, "old@x.com").unwrap().unwrap();
        assert_eq!(old.first_name, "Old", "blank cell keeps existing value");
        assert_eq!(old.fields["company"], "New Co");
        assert_eq!(list_member_ids(&conn, list).unwrap().len(), 2);
        assert_eq!(list_tags(&conn).unwrap(), vec!["Conference"]);
    }

    #[test]
    fn import_requires_email_column() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.lock();
        let t = table(&[&["Name"], &["Ana"]]);
        let req = ImportRequest { mapping: vec![Target::FirstName], list_id: None, tags: vec![], to_dnc: false };
        assert!(import(&mut conn, &t, &req, now()).is_err());
    }

    #[test]
    fn import_to_do_not_contact() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.lock();
        let t = table(&[&["email"], &["A@x.com"], &["b@x.com"], &["a@x.com"]]);
        let req = ImportRequest { mapping: vec![Target::Email], list_id: None, tags: vec![], to_dnc: true };
        let s = import(&mut conn, &t, &req, now()).unwrap();
        assert_eq!((s.added, s.duplicates), (2, 1));
        assert!(is_dnc(&conn, " a@X.com").unwrap());
        remove_dnc(&conn, "A@x.com").unwrap();
        assert!(!is_dnc(&conn, "a@x.com").unwrap());
    }

    #[test]
    fn manual_save_rejects_duplicates_and_bad_addresses() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let mk = |e: &str| Contact { id: 0, email: e.into(), first_name: String::new(), last_name: String::new(),
            fields: BTreeMap::new(), tags: vec!["VIP".into()], lists: vec![], do_not_contact: false, created_at: String::new() };
        let a = save(&conn, &mk("a@x.com"), now()).unwrap();
        assert_eq!(a.tags, vec!["VIP"]);
        assert!(save(&conn, &mk("A@X.com"), now()).is_err());
        assert!(save(&conn, &mk("nope"), now()).is_err());
        let page = query(&conn, &ContactQuery { search: "A@x".into(), tag: Some("VIP".into()), ..Default::default() }).unwrap();
        assert_eq!(page.total, 1);
        delete(&conn, &[a.id]).unwrap();
        assert_eq!(list_tags(&conn).unwrap().len(), 1, "tags table is tidied on next set_tags only");
    }
}
