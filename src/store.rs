use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::error::Error;
use crate::index::INDEX_VERSION;
use rusqlite::{Connection, OptionalExtension};

fn as_bytes(floats: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(floats.as_ptr() as *const u8, std::mem::size_of_val(floats))
    }
}

static SQLITE_VEC_INIT: Once = Once::new();

#[allow(clippy::missing_transmute_annotations)]
fn ensure_sqlite_vec_loaded() {
    SQLITE_VEC_INIT.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sources (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    source_path TEXT NOT NULL REFERENCES sources(path),
    heading TEXT NOT NULL,
    slug TEXT NOT NULL,
    parent_id INTEGER REFERENCES chunks(id),
    position INTEGER NOT NULL,
    content TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_slug ON chunks(slug);
CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_path);
CREATE INDEX IF NOT EXISTS idx_chunks_parent ON chunks(parent_id);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

const SCHEMA_VEC: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
    chunk_id INTEGER PRIMARY KEY,
    embedding float[256]
);
";

const CHUNK_COLUMNS: &str = "id, source_path, heading, slug, parent_id, position, content";

pub struct Store {
    root: PathBuf,
    db: Connection,
}

impl Store {
    /// If not found, creates `.stacks/` at the git repo root (or cwd).
    pub fn find_or_create(start: &Path) -> Result<Self, Error> {
        let mut dir = start.to_path_buf();
        loop {
            let candidate = dir.join(".stacks");
            if candidate.is_dir() {
                return Self::open(candidate);
            }
            if dir.join(".git").exists() {
                let stacks_dir = dir.join(".stacks");
                fs::create_dir_all(&stacks_dir)?;
                return Self::open(stacks_dir);
            }
            if !dir.pop() {
                break;
            }
        }
        let stacks_dir = start.join(".stacks");
        fs::create_dir_all(&stacks_dir)?;
        Self::open(stacks_dir)
    }

    fn open(root: PathBuf) -> Result<Self, Error> {
        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            fs::write(&gitignore, "*\n")?;
        }

        ensure_sqlite_vec_loaded();

        let db_path = root.join("index.db");
        let db = Connection::open(&db_path)?;
        db.execute_batch("PRAGMA foreign_keys = ON;")?;
        db.execute_batch(SCHEMA)?;
        db.execute_batch(SCHEMA_VEC)?;

        let store = Store { root, db };
        store.check_index_version()?;
        Ok(store)
    }

    fn check_index_version(&self) -> Result<(), Error> {
        let version_str = INDEX_VERSION.to_string();
        let stored: Option<String> = self
            .db
            .query_row(
                "SELECT value FROM metadata WHERE key = 'index_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(&version_str) {
            self.clear_all()?;
            self.db.execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES ('index_version', ?1)",
                [&version_str],
            )?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_root(&self) -> &Path {
        self.root.parent().expect(".stacks has no parent")
    }

    pub fn transaction(&self, f: impl FnOnce(&Self) -> Result<(), Error>) -> Result<(), Error> {
        self.db.execute_batch("BEGIN")?;
        match f(self) {
            Ok(()) => {
                self.db.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.db.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn upsert_source(&self, path: &str, hash: &str) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO sources (path, content_hash) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET content_hash = ?2",
            [path, hash],
        )?;
        Ok(())
    }

    pub fn delete_chunks_for_source(&self, path: &str) -> Result<(), Error> {
        self.delete_embeddings_for_source(path)?;
        self.db
            .execute("DELETE FROM chunks WHERE source_path = ?1", [path])?;
        Ok(())
    }

    pub fn insert_chunk(&self, chunk: &NewChunk) -> Result<i64, Error> {
        self.db.execute(
            "INSERT INTO chunks (source_path, heading, slug, parent_id, position, content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                chunk.source_path,
                chunk.heading,
                chunk.slug,
                chunk.parent_id,
                chunk.position,
                chunk.content
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn slug_exists(&self, slug: &str) -> Result<bool, Error> {
        self.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chunks WHERE slug = ?1)",
                [slug],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn all_sources(&self) -> Result<HashMap<String, String>, Error> {
        let mut stmt = self.db.prepare("SELECT path, content_hash FROM sources")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }

    pub fn delete_source(&self, path: &str) -> Result<(), Error> {
        self.delete_chunks_for_source(path)?;
        self.db
            .execute("DELETE FROM sources WHERE path = ?1", [path])?;
        Ok(())
    }

    pub fn all_chunks(&self) -> Result<Vec<Chunk>, Error> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT {CHUNK_COLUMNS} FROM chunks ORDER BY source_path, position"
        ))?;
        let rows = stmt.query_map([], Chunk::from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn chunk_by_slug(&self, slug: &str) -> Result<Option<Chunk>, Error> {
        self.db
            .query_row(
                &format!("SELECT {CHUNK_COLUMNS} FROM chunks WHERE slug = ?1"),
                [slug],
                Chunk::from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn children_of(&self, parent_id: i64) -> Result<Vec<Chunk>, Error> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT {CHUNK_COLUMNS} FROM chunks WHERE parent_id = ?1 ORDER BY position"
        ))?;
        let rows = stmt.query_map([parent_id], Chunk::from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn root_chunks_for_source(&self, source_path: &str) -> Result<Vec<Chunk>, Error> {
        let mut stmt = self.db.prepare(&format!(
            "SELECT {CHUNK_COLUMNS} FROM chunks WHERE source_path = ?1 AND parent_id IS NULL ORDER BY position"
        ))?;
        let rows = stmt.query_map([source_path], Chunk::from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Ancestor chain from the database, excluding the root (file-level) chunk.
    pub fn ancestors_by_query(&self, chunk: &Chunk) -> Result<Vec<Chunk>, Error> {
        let mut ancestors = Vec::new();
        let mut current_parent = chunk.parent_id;
        while let Some(pid) = current_parent {
            let parent: Chunk = self.db.query_row(
                &format!("SELECT {CHUNK_COLUMNS} FROM chunks WHERE id = ?1"),
                [pid],
                Chunk::from_row,
            )?;
            current_parent = parent.parent_id;
            ancestors.push(parent);
        }
        ancestors.reverse();
        strip_root_ancestor(&mut ancestors);
        Ok(ancestors)
    }

    pub fn subtree(&self, root_id: i64) -> Result<Vec<(Chunk, i32)>, Error> {
        let mut stmt = self.db.prepare(&format!(
            "WITH RECURSIVE subtree AS (
                SELECT {CHUNK_COLUMNS}, 0 AS depth FROM chunks WHERE id = ?1
                UNION ALL
                SELECT c.id, c.source_path, c.heading, c.slug, c.parent_id, c.position, c.content, s.depth + 1
                FROM chunks c JOIN subtree s ON c.parent_id = s.id
            )
            SELECT {CHUNK_COLUMNS}, depth FROM subtree ORDER BY source_path, position"
        ))?;
        let rows = stmt.query_map([root_id], |row| Ok((Chunk::from_row(row)?, row.get(7)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn insert_embedding(&self, chunk_id: i64, embedding: &[f32]) -> Result<(), Error> {
        self.db.execute(
            "INSERT INTO chunks_vec (chunk_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![chunk_id, as_bytes(embedding)],
        )?;
        Ok(())
    }

    pub fn delete_embeddings_for_source(&self, path: &str) -> Result<(), Error> {
        self.db.execute(
            "DELETE FROM chunks_vec WHERE chunk_id IN (SELECT id FROM chunks WHERE source_path = ?1)",
            [path],
        )?;
        Ok(())
    }

    pub fn search_similar(
        &self,
        query: &[f32],
        limit: usize,
        path: Option<&str>,
    ) -> Result<Vec<(Chunk, f64)>, Error> {
        // Over-fetch 5x when path-scoping: KNN can't filter by source_path
        // in the virtual table query, so we fetch extra and post-filter.
        let fetch_limit = if path.is_some() { limit * 5 } else { limit };
        let mut stmt = self.db.prepare(
            "SELECT c.id, c.source_path, c.heading, c.slug, c.parent_id, c.position, c.content,
                    v.distance
             FROM chunks_vec v JOIN chunks c ON c.id = v.chunk_id
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        )?;
        let rows = stmt.query_map(rusqlite::params![as_bytes(query), fetch_limit], |row| {
            Ok((Chunk::from_row(row)?, row.get(7)?))
        })?;
        let results: Vec<(Chunk, f64)> = rows.collect::<Result<Vec<_>, _>>()?;
        match path {
            Some(p) => Ok(results
                .into_iter()
                .filter(|(c, _)| c.source_path.starts_with(p))
                .take(limit)
                .collect()),
            None => Ok(results),
        }
    }

    /// True if chunks exist but have no embeddings (needs full re-index).
    pub fn needs_embedding(&self) -> Result<bool, Error> {
        let chunk_count: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        let vec_count: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM chunks_vec", [], |row| row.get(0))?;
        Ok(chunk_count > 0 && vec_count == 0)
    }

    pub fn clear_all(&self) -> Result<(), Error> {
        self.db
            .execute_batch("DELETE FROM chunks_vec; DELETE FROM chunks; DELETE FROM sources;")?;
        Ok(())
    }
}

pub struct NewChunk<'a> {
    pub source_path: &'a str,
    pub heading: &'a str,
    pub slug: &'a str,
    pub parent_id: Option<i64>,
    pub position: i32,
    pub content: &'a str,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // position is used for SQL ordering, not read in Rust
pub struct Chunk {
    pub id: i64,
    pub source_path: String,
    pub heading: String,
    pub slug: String,
    pub parent_id: Option<i64>,
    pub position: i32,
    pub content: String,
}

/// Build ancestor chain from an in-memory chunk map, excluding root chunk.
pub fn ancestors_from_chunks<'a>(
    chunk: &Chunk,
    id_to_chunk: &'a HashMap<i64, &'a Chunk>,
) -> Vec<Chunk> {
    let mut ancestors = Vec::new();
    let mut pid = chunk.parent_id;
    while let Some(p) = pid {
        if let Some(parent) = id_to_chunk.get(&p) {
            pid = parent.parent_id;
            ancestors.push((*parent).clone());
        } else {
            break;
        }
    }
    ancestors.reverse();
    strip_root_ancestor(&mut ancestors);
    ancestors
}

/// Remove the root (file-level) chunk from an ancestor chain — callers only need heading ancestors.
fn strip_root_ancestor(ancestors: &mut Vec<Chunk>) {
    if ancestors.first().is_some_and(|a| a.parent_id.is_none()) {
        ancestors.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: i64, parent_id: Option<i64>) -> Chunk {
        Chunk {
            id,
            source_path: String::new(),
            heading: format!("H{id}"),
            slug: String::new(),
            parent_id,
            position: 0,
            content: String::new(),
        }
    }

    #[test]
    fn strip_root_ancestor_removes_file_level_chunk() {
        let mut ancestors = vec![chunk(1, None), chunk(2, Some(1)), chunk(3, Some(2))];
        strip_root_ancestor(&mut ancestors);
        assert_eq!(ancestors.len(), 2);
        assert_eq!(ancestors[0].id, 2);
    }

    #[test]
    fn strip_root_ancestor_keeps_non_root() {
        let mut ancestors = vec![chunk(2, Some(1)), chunk(3, Some(2))];
        strip_root_ancestor(&mut ancestors);
        assert_eq!(ancestors.len(), 2);
    }

    #[test]
    fn strip_root_ancestor_empty() {
        let mut ancestors: Vec<Chunk> = vec![];
        strip_root_ancestor(&mut ancestors);
        assert!(ancestors.is_empty());
    }
}

impl Chunk {
    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        Ok(Chunk {
            id: row.get(0)?,
            source_path: row.get(1)?,
            heading: row.get(2)?,
            slug: row.get(3)?,
            parent_id: row.get(4)?,
            position: row.get(5)?,
            content: row.get(6)?,
        })
    }
}
