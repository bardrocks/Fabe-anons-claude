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
pub struct Ranked {
    pub person_id: Option<i64>,
    pub name: String,
    pub count: i64,
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
-- Default categories only on a fresh database, so deleted/renamed ones don't come back.
INSERT INTO categories(name, position)
  SELECT * FROM (VALUES ('Personeller', 1), ('Firmalar', 2)) WHERE NOT EXISTS (SELECT 1 FROM categories);
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

    pub fn rename_category(&self, id: i64, new_name: &str) -> Result<()> {
        let new_name = new_name.trim();
        anyhow::ensure!(!new_name.is_empty(), "Kategori adı boş olamaz");
        let mut c = self.c();
        let tx = c.transaction()?;
        let old: String = tx.query_row("SELECT name FROM categories WHERE id=?1", [id], |r| r.get(0))?;
        let taken: i64 = tx.query_row(
            "SELECT COUNT(*) FROM categories WHERE name=?1 AND id<>?2",
            params![new_name, id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(taken == 0, "\"{new_name}\" adında bir kategori zaten var");
        tx.execute("UPDATE categories SET name=?1 WHERE id=?2", params![new_name, id])?;
        tx.execute("UPDATE people SET category=?1 WHERE category=?2 COLLATE NOCASE", params![new_name, old])?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes a category; its people move to the first remaining category.
    /// Returns how many people were moved and where.
    pub fn delete_category(&self, id: i64) -> Result<(usize, String)> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let n: i64 = tx.query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))?;
        anyhow::ensure!(n > 1, "Son kategori silinemez");
        let old: String = tx.query_row("SELECT name FROM categories WHERE id=?1", [id], |r| r.get(0))?;
        tx.execute("DELETE FROM categories WHERE id=?1", [id])?;
        let target: String =
            tx.query_row("SELECT name FROM categories ORDER BY position, id LIMIT 1", [], |r| r.get(0))?;
        let moved = tx.execute("UPDATE people SET category=?1 WHERE category=?2 COLLATE NOCASE", params![target, old])?;
        tx.commit()?;
        Ok((moved, target))
    }

    /// Moves a category one step left (-1) or right (+1) in the tab order.
    pub fn move_category(&self, id: i64, step: i64) -> Result<()> {
        let cats = self.categories()?;
        let Some(i) = cats.iter().position(|c| c.id == id) else { anyhow::bail!("Kategori bulunamadı") };
        let j = i as i64 + step.signum();
        if j < 0 || j as usize >= cats.len() {
            return Ok(());
        }
        let mut order: Vec<i64> = cats.iter().map(|c| c.id).collect();
        order.swap(i, j as usize);
        let mut c = self.c();
        let tx = c.transaction()?;
        for (pos, cid) in order.iter().enumerate() {
            tx.execute("UPDATE categories SET position=?1 WHERE id=?2", params![pos as i64 + 1, cid])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Edits a person's button. `sound_file` None keeps the current sound.
    pub fn update_person(
        &self,
        id: i64,
        name: &str,
        category: &str,
        color: Option<&str>,
        sound_file: Option<&str>,
    ) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "İsim boş olamaz");
        let cat = self.add_category(category)?;
        let c = self.c();
        let taken: i64 =
            c.query_row("SELECT COUNT(*) FROM people WHERE name=?1 AND id<>?2", params![name, id], |r| r.get(0))?;
        anyhow::ensure!(taken == 0, "\"{name}\" adında biri zaten var");
        c.execute(
            "UPDATE people SET name=?1, category=?2, color=?3, sound_file=COALESCE(?4, sound_file) WHERE id=?5",
            params![name, cat.name, color, sound_file, id],
        )?;
        Ok(())
    }

    pub fn create_person(&self, name: &str, category: &str, color: Option<&str>, sound_file: &str) -> Result<i64> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "İsim boş olamaz");
        let exists: i64 = self.c().query_row("SELECT COUNT(*) FROM people WHERE name=?1", [name], |r| r.get(0))?;
        anyhow::ensure!(exists == 0, "\"{name}\" adında biri zaten var");
        self.add_person(name, category, color, Some(sound_file), 0)
    }

    /// Deletes a person. Their past announcements stay in the history.
    pub fn delete_person(&self, id: i64) -> Result<Option<String>> {
        let c = self.c();
        let sound: Option<String> =
            c.query_row("SELECT sound_file FROM people WHERE id=?1", [id], |r| r.get(0)).optional()?.flatten();
        c.execute("UPDATE calls SET person_id=NULL WHERE person_id=?1", [id])?;
        c.execute("DELETE FROM people WHERE id=?1", [id])?;
        Ok(sound)
    }

    /// True if a sound file is still used by a person or a break (so it must not be deleted).
    pub fn sound_in_use(&self, file: &str) -> Result<bool> {
        let n: i64 = self.c().query_row(
            "SELECT (SELECT COUNT(*) FROM people WHERE sound_file=?1) + (SELECT COUNT(*) FROM breaks WHERE sound_file=?1)",
            [file],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn update_break(&self, id: i64, time: &str, sound_file: Option<&str>, label: &str) -> Result<()> {
        self.c().execute(
            "UPDATE breaks SET time=?1, sound_file=COALESCE(?2, sound_file), label=?3 WHERE id=?4",
            params![time, sound_file, label, id],
        )?;
        Ok(())
    }

    pub fn delete_break(&self, id: i64) -> Result<Option<String>> {
        let c = self.c();
        let sound: Option<String> =
            c.query_row("SELECT sound_file FROM breaks WHERE id=?1", [id], |r| r.get(0)).optional()?;
        c.execute("DELETE FROM breaks WHERE id=?1", [id])?;
        Ok(sound)
    }

    /// Writes a consistent copy of the whole database to `dest`.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        let _ = std::fs::remove_file(dest);
        self.c().backup(rusqlite::DatabaseName::Main, dest, None)?;
        Ok(())
    }

    /// Replaces the whole database with the one in `src` (restore from backup).
    pub fn restore_from(&self, src: &Path) -> Result<()> {
        // Make sure it really is a FABE database before touching ours.
        let check = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        check.query_row("SELECT COUNT(*) FROM people", [], |r| r.get::<_, i64>(0))?;
        drop(check);
        let mut c = self.c();
        c.restore(rusqlite::DatabaseName::Main, src, None::<fn(rusqlite::backup::Progress)>)?;
        c.execute_batch(SCHEMA)?; // adds tables a newer version expects
        Ok(())
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

    /// Most-called names since `since` ("YYYY-MM-DD HH:MM:SS", local time).
    pub fn top_called(&self, since: &str, limit: i64) -> Result<Vec<Ranked>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT MAX(person_id), title, COUNT(*) AS n FROM calls
             WHERE kind = 'person' AND at >= ?1
             GROUP BY COALESCE(person_id, -1), CASE WHEN person_id IS NULL THEN title ELSE '' END
             ORDER BY n DESC, title LIMIT ?2",
        )?;
        let rows = st.query_map(params![since, limit], |r| Ok(Ranked { person_id: r.get(0)?, name: r.get(1)?, count: r.get(2)? }))?;
        let mut out: Vec<Ranked> = rows.collect::<std::result::Result<_, _>>()?;
        // Show the person's current name if they were renamed since.
        for r in &mut out {
            if let Some(id) = r.person_id {
                if let Ok(name) = c.query_row("SELECT name FROM people WHERE id = ?1", [id], |x| x.get::<_, String>(0)) {
                    r.name = name;
                }
            }
        }
        Ok(out)
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
    fn top_called_counts_only_the_period_and_uses_current_names() {
        let s = Store::memory().unwrap();
        let a = s.add_person("Ayşe", "Personeller", None, Some("ayse.mp3"), 50).unwrap();
        let b = s.add_person("Eren", "Personeller", None, Some("eren.mp3"), 0).unwrap();
        for _ in 0..2 {
            s.log_call(Some(b), "person", "Eren", "Bu bilgisayar").unwrap();
        }
        s.log_call(Some(a), "person", "Ayşe", "Bu bilgisayar").unwrap();
        s.log_call(None, "person", "Misafir", "KASA-2").unwrap();
        s.log_call(None, "tts", "Araç sahibi", "Bu bilgisayar").unwrap();
        s.c().execute("UPDATE calls SET at = '2020-01-01 10:00:00' WHERE person_id = ?1", [a]).unwrap();
        s.update_person(b, "Eren Kaya", "Personeller", None, None).unwrap();
        let top = s.top_called("2026-01-01 00:00:00", 5).unwrap();
        let names: Vec<_> = top.iter().map(|r| (r.name.as_str(), r.count)).collect();
        assert_eq!(names, [("Eren Kaya", 2), ("Misafir", 1)]);
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
