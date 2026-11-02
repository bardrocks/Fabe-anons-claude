//! SQLite storage: people, call log, scheduled breaks and settings.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: String,
    /// Category name, e.g. "Personeller", "Firmalar", "Depo".
    pub category: String,
    pub color: Option<String>,
    pub sound_file: Option<String>,
    /// Calls counted by the old app before migration plus calls logged since.
    pub count: i64,
    pub today: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEntry {
    pub id: i64,
    pub at: String,
    pub title: String,
    pub kind: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakEntry {
    pub id: i64,
    pub time: String,
    pub sound_file: String,
    pub label: String,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS people (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    category    TEXT NOT NULL DEFAULT 'Personeller',
    color       TEXT,
    sound_file  TEXT,
    legacy_calls INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE TABLE IF NOT EXISTS calls (
    id        INTEGER PRIMARY KEY,
    person_id INTEGER REFERENCES people(id) ON DELETE SET NULL,
    kind      TEXT NOT NULL,          -- 'person' | 'tts' | 'break'
    title     TEXT NOT NULL,
    source    TEXT NOT NULL,          -- which computer sent it
    at        TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE INDEX IF NOT EXISTS calls_at ON calls(at);
CREATE TABLE IF NOT EXISTS breaks (
    id         INTEGER PRIMARY KEY,
    time       TEXT NOT NULL,         -- HH:MM
    sound_file TEXT NOT NULL,
    label      TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS categories (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL UNIQUE COLLATE NOCASE,
    position INTEGER NOT NULL
);
INSERT OR IGNORE INTO categories(name, position) VALUES ('Personeller', 1), ('Firmalar', 2);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    #[cfg(test)]
    fn person_id_for_test(&self, name: &str) -> i64 {
        self.c().query_row("SELECT id FROM people WHERE name=?1", [name], |r| r.get(0)).unwrap()
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.c().query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))?;
        Ok(n == 0)
    }

    pub fn add_person(
        &self,
        name: &str,
        category: &str,
        color: Option<&str>,
        sound_file: Option<&str>,
        legacy_calls: i64,
    ) -> Result<i64> {
        self.add_category(category)?;
        let c = self.c();
        c.execute(
            "INSERT INTO people(name, category, color, sound_file, legacy_calls) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(name) DO UPDATE SET category=excluded.category, color=excluded.color,
               sound_file=COALESCE(excluded.sound_file, people.sound_file)",
            params![name, category, color, sound_file, legacy_calls],
        )?;
        Ok(c.query_row("SELECT id FROM people WHERE name=?1", [name], |r| r.get(0))?)
    }

    /// Adds a user-defined category (e.g. "Depo"). Adding an existing name does nothing.
    pub fn add_category(&self, name: &str) -> Result<Category> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "Kategori adı boş olamaz");
        let c = self.c();
        c.execute(
            "INSERT OR IGNORE INTO categories(name, position)
             VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM categories))",
            [name],
        )?;
        Ok(c.query_row("SELECT id, name, position FROM categories WHERE name=?1", [name], |r| {
            Ok(Category { id: r.get(0)?, name: r.get(1)?, position: r.get(2)? })
        })?)
    }

    pub fn set_person_category(&self, person_id: i64, category: &str) -> Result<()> {
        let cat = self.add_category(category)?;
        self.c().execute("UPDATE people SET category=?1 WHERE id=?2", params![cat.name, person_id])?;
        Ok(())
    }

    pub fn categories(&self) -> Result<Vec<Category>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, name, position FROM categories ORDER BY position, id")?;
        let rows = st.query_map([], |r| Ok(Category { id: r.get(0)?, name: r.get(1)?, position: r.get(2)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn people(&self) -> Result<Vec<Person>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT p.id, p.name, p.category, p.color, p.sound_file,
                    p.legacy_calls + (SELECT COUNT(*) FROM calls WHERE person_id = p.id),
                    (SELECT COUNT(*) FROM calls WHERE person_id = p.id AND date(at) = date('now','localtime'))
             FROM people p",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Person {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
                color: r.get(3)?,
                sound_file: r.get(4)?,
                count: r.get(5)?,
                today: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn person(&self, id: i64) -> Result<Option<Person>> {
        Ok(self.people()?.into_iter().find(|p| p.id == id))
    }

    pub fn log_call(&self, person_id: Option<i64>, kind: &str, title: &str, source: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO calls(person_id, kind, title, source) VALUES (?1,?2,?3,?4)",
            params![person_id, kind, title, source],
        )?;
        Ok(())
    }

    pub fn history(&self, limit: i64) -> Result<Vec<CallEntry>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, at, title, kind, source FROM calls ORDER BY id DESC LIMIT ?1")?;
        let rows = st.query_map([limit], |r| {
            Ok(CallEntry { id: r.get(0)?, at: r.get(1)?, title: r.get(2)?, kind: r.get(3)?, source: r.get(4)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn add_break(&self, time: &str, sound_file: &str, label: &str) -> Result<()> {
        // Importing the same backup twice must not create duplicate breaks.
        self.c().execute(
            "INSERT INTO breaks(time, sound_file, label)
             SELECT ?1,?2,?3 WHERE NOT EXISTS (SELECT 1 FROM breaks WHERE time=?1 AND sound_file=?2)",
            params![time, sound_file, label],
        )?;
        Ok(())
    }

    pub fn breaks(&self) -> Result<Vec<BreakEntry>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, time, sound_file, label FROM breaks ORDER BY time")?;
        let rows = st.query_map([], |r| {
            Ok(BreakEntry { id: r.get(0)?, time: r.get(1)?, sound_file: r.get(2)?, label: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .c()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn setting_f32(&self, key: &str, default: f32) -> f32 {
        self.setting(key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_include_legacy_and_logged_calls() {
        let s = Store::memory().unwrap();
        let id = s.add_person("Ayşe", "Personeller", None, Some("ayse.mp3"), 10).unwrap();
        s.log_call(Some(id), "person", "Ayşe", "Bu bilgisayar").unwrap();
        let p = s.person(id).unwrap().unwrap();
        assert_eq!(p.count, 11);
        assert_eq!(p.today, 1);
        assert_eq!(s.history(5).unwrap()[0].title, "Ayşe");
    }

    #[test]
    fn user_categories_keep_order_and_ignore_duplicates() {
        let s = Store::memory().unwrap();
        s.add_category("Depo").unwrap();
        s.add_category("depo ").unwrap();
        s.add_person("Ali", "Kasa", None, None, 0).unwrap();
        let names: Vec<_> = s.categories().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Personeller", "Firmalar", "Depo", "Kasa"]);
        assert!(s.add_category("  ").is_err());
        let id = s.person_id_for_test("Ali");
        s.set_person_category(id, "DEPO").unwrap();
        assert_eq!(s.person(id).unwrap().unwrap().category, "Depo");
    }

    #[test]
    fn re_adding_keeps_existing_sound() {
        let s = Store::memory().unwrap();
        s.add_person("Eren", "Personeller", None, Some("eren.mp3"), 0).unwrap();
        s.add_person("Eren", "Personeller", Some("#2E7D32"), None, 0).unwrap();
        let p = &s.people().unwrap()[0];
        assert_eq!(p.sound_file.as_deref(), Some("eren.mp3"));
        assert_eq!(p.color.as_deref(), Some("#2E7D32"));
    }
}
