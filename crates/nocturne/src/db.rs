//! Biblioteca, historial y estadísticas en SQLite (`PRAGMA user_version` para migraciones).
//! La música no se guarda en la base: solo la ruta y la huella. Los mapas viven en archivos JSON.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status { Analyzing, Ready, Failed, Cancelled }

impl Status {
    fn as_str(self) -> &'static str { match self { Self::Analyzing => "analyzing", Self::Ready => "ready", Self::Failed => "failed", Self::Cancelled => "cancelled" } }
    fn parse(s: &str) -> Self { match s { "ready" => Self::Ready, "failed" => Self::Failed, "cancelled" => Self::Cancelled, _ => Self::Analyzing } }
}

#[derive(Debug, Clone)]
pub struct Song {
    pub id: i64,
    pub fingerprint: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub duration_ms: i64,
    pub bpm: Option<f64>,
    pub status: Status,
    pub error: Option<String>,
    pub added: i64,
    pub last_played: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ScoreRow {
    pub fingerprint: String,
    pub difficulty: String,
    pub score: u32,
    pub accuracy: f64,
    pub max_combo: u32,
    pub counts: [u32; 6],
    pub mean_error_ms: Option<f64>,
    pub early: u32,
    pub late: u32,
    /// Histograma de errores firmados (bins de 10 ms de −150 a +150 → 30 casillas).
    pub histogram: Vec<u32>,
    pub played_at: i64,
    pub duration_ms: i64,
    pub failed: bool,
}

pub struct Db { conn: Connection }

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Db {
    pub fn open_memory() -> rusqlite::Result<Db> { Self::init(Connection::open_in_memory()?) }

    /// Abre la base. Si está corrupta se aparta como `.corrupt` y se crea una nueva (devuelve aviso).
    pub fn open(path: &Path) -> (Db, Option<String>) {
        if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
        match Connection::open(path).and_then(Self::init) {
            Ok(db) => (db, None),
            Err(e) => {
                let bak = path.with_extension("db.corrupt");
                let _ = std::fs::rename(path, &bak);
                let db = Connection::open(path).and_then(Self::init)
                    .or_else(|_| Self::open_memory().map(|d| d.conn).and_then(Self::init)).expect("sqlite en memoria");
                (db, Some(format!("base de datos ilegible ({e}); se movió a {} y se creó una nueva", bak.display())))
            }
        }
    }

    fn init(conn: Connection) -> rusqlite::Result<Db> {
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "foreign_keys", "ON");
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let v: i32 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v > SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidParameterName(format!("esquema {v} más nuevo que el soportado ({SCHEMA_VERSION})")));
        }
        if v < 1 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch("
                CREATE TABLE songs(
                    id INTEGER PRIMARY KEY, fingerprint TEXT NOT NULL UNIQUE, path TEXT NOT NULL,
                    title TEXT NOT NULL, artist TEXT NOT NULL DEFAULT '', duration_ms INTEGER NOT NULL DEFAULT 0,
                    bpm REAL, status TEXT NOT NULL, error TEXT, added INTEGER NOT NULL, last_played INTEGER);
                CREATE TABLE scores(
                    id INTEGER PRIMARY KEY, fingerprint TEXT NOT NULL, difficulty TEXT NOT NULL, score INTEGER NOT NULL,
                    accuracy REAL NOT NULL, max_combo INTEGER NOT NULL, counts TEXT NOT NULL, mean_error REAL,
                    early INTEGER NOT NULL, late INTEGER NOT NULL, histogram TEXT NOT NULL, played_at INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL, failed INTEGER NOT NULL);
                CREATE INDEX scores_song ON scores(fingerprint, difficulty);
                CREATE INDEX scores_time ON scores(played_at);
            ")?;
            tx.commit()?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> i32 { self.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap_or(0) }

    // --- Biblioteca -------------------------------------------------------------------------

    fn row_song(r: &rusqlite::Row) -> rusqlite::Result<Song> {
        Ok(Song { id: r.get(0)?, fingerprint: r.get(1)?, path: r.get(2)?, title: r.get(3)?, artist: r.get(4)?,
            duration_ms: r.get(5)?, bpm: r.get(6)?, status: Status::parse(&r.get::<_, String>(7)?), error: r.get(8)?,
            added: r.get(9)?, last_played: r.get(10)? })
    }
    const SONG_COLS: &'static str = "id,fingerprint,path,title,artist,duration_ms,bpm,status,error,added,last_played";

    pub fn songs(&self) -> rusqlite::Result<Vec<Song>> {
        let mut st = self.conn.prepare(&format!("SELECT {} FROM songs ORDER BY added DESC", Self::SONG_COLS))?;
        let v = st.query_map([], Self::row_song)?.collect();
        v
    }
    pub fn song_by_fingerprint(&self, fp: &str) -> rusqlite::Result<Option<Song>> {
        self.conn.query_row(&format!("SELECT {} FROM songs WHERE fingerprint=?1", Self::SONG_COLS), [fp], Self::row_song).optional()
    }
    pub fn song(&self, id: i64) -> rusqlite::Result<Option<Song>> {
        self.conn.query_row(&format!("SELECT {} FROM songs WHERE id=?1", Self::SONG_COLS), [id], Self::row_song).optional()
    }
    /// Inserta una canción en estado «analizando». Error de unicidad si la huella ya existe.
    pub fn add_song(&self, fp: &str, path: &str, title: &str, artist: &str, duration_ms: i64) -> rusqlite::Result<i64> {
        self.conn.execute("INSERT INTO songs(fingerprint,path,title,artist,duration_ms,status,added) VALUES(?1,?2,?3,?4,?5,'analyzing',?6)",
            params![fp, path, title, artist, duration_ms, now()])?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn set_status(&self, id: i64, status: Status, error: Option<&str>, bpm: Option<f64>) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE songs SET status=?2, error=?3, bpm=COALESCE(?4,bpm) WHERE id=?1", params![id, status.as_str(), error, bpm])?;
        Ok(())
    }
    pub fn update_path(&self, id: i64, path: &str) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE songs SET path=?2 WHERE id=?1", params![id, path])?; Ok(())
    }
    pub fn touch_played(&self, fp: &str) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE songs SET last_played=?2 WHERE fingerprint=?1", params![fp, now()])?; Ok(())
    }
    pub fn delete_song(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM songs WHERE id=?1", [id])?; Ok(())
    }
    /// Canciones que quedaron «analizando» por un cierre brusco → se marcan fallidas al arrancar.
    pub fn recover_interrupted(&self) -> rusqlite::Result<usize> {
        self.conn.execute("UPDATE songs SET status='failed', error='análisis interrumpido' WHERE status='analyzing'", [])
    }
    pub fn last_played_song(&self) -> rusqlite::Result<Option<Song>> {
        self.conn.query_row(&format!("SELECT {} FROM songs WHERE last_played IS NOT NULL ORDER BY last_played DESC LIMIT 1", Self::SONG_COLS), [], Self::row_song).optional()
    }

    // --- Resultados ---------------------------------------------------------------------------

    pub fn add_score(&self, s: &ScoreRow) -> rusqlite::Result<()> {
        self.conn.execute("INSERT INTO scores(fingerprint,difficulty,score,accuracy,max_combo,counts,mean_error,early,late,histogram,played_at,duration_ms,failed)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![s.fingerprint, s.difficulty, s.score, s.accuracy, s.max_combo, serde_json::to_string(&s.counts).unwrap(),
                s.mean_error_ms, s.early, s.late, serde_json::to_string(&s.histogram).unwrap(), s.played_at, s.duration_ms, s.failed as i32])?;
        Ok(())
    }

    fn row_score(r: &rusqlite::Row) -> rusqlite::Result<ScoreRow> {
        let counts: [u32; 6] = serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or([0; 6]);
        let histogram: Vec<u32> = serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default();
        Ok(ScoreRow { fingerprint: r.get(0)?, difficulty: r.get(1)?, score: r.get(2)?, accuracy: r.get(3)?, max_combo: r.get(4)?,
            counts, mean_error_ms: r.get(6)?, early: r.get(7)?, late: r.get(8)?, histogram, played_at: r.get(10)?,
            duration_ms: r.get(11)?, failed: r.get::<_, i32>(12)? != 0 })
    }
    const SCORE_COLS: &'static str = "fingerprint,difficulty,score,accuracy,max_combo,counts,mean_error,early,late,histogram,played_at,duration_ms,failed";

    /// Mejor puntuación (sin partidas falladas) de una canción y dificultad.
    pub fn best(&self, fp: &str, diff: &str) -> rusqlite::Result<Option<ScoreRow>> {
        self.conn.query_row(&format!("SELECT {} FROM scores WHERE fingerprint=?1 AND difficulty=?2 AND failed=0 ORDER BY score DESC LIMIT 1", Self::SCORE_COLS),
            params![fp, diff], Self::row_score).optional()
    }
    pub fn recent(&self, limit: usize) -> rusqlite::Result<Vec<ScoreRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {} FROM scores ORDER BY played_at DESC, id DESC LIMIT ?1", Self::SCORE_COLS))?;
        let v = st.query_map([limit as i64], Self::row_score)?.collect();
        v
    }
    /// (partidas, tiempo jugado ms, precisión media, mejor combo)
    pub fn totals(&self) -> rusqlite::Result<(i64, i64, Option<f64>, Option<i64>)> {
        self.conn.query_row("SELECT COUNT(*), COALESCE(SUM(duration_ms),0), AVG(accuracy), MAX(max_combo) FROM scores", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
    }
    /// Histograma de errores agregado de todas las partidas.
    pub fn error_histogram(&self) -> rusqlite::Result<Vec<u32>> {
        let mut total = vec![0u32; 30];
        let mut st = self.conn.prepare("SELECT histogram FROM scores")?;
        for h in st.query_map([], |r| r.get::<_, String>(0))? {
            if let Ok(v) = serde_json::from_str::<Vec<u32>>(&h?) { for (i, c) in v.iter().enumerate().take(30) { total[i] += c } }
        }
        Ok(total)
    }
}

/// Histograma de 30 casillas de 10 ms entre −150 y +150 ms (los extremos acumulan el resto).
pub fn histogram(errors_ms: &[f32]) -> Vec<u32> {
    let mut h = vec![0u32; 30];
    for e in errors_ms { h[(((e + 150.0) / 10.0).floor() as isize).clamp(0, 29) as usize] += 1 }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(fp: &str, diff: &str, s: u32, failed: bool) -> ScoreRow {
        ScoreRow { fingerprint: fp.into(), difficulty: diff.into(), score: s, accuracy: 90.0, max_combo: 50, counts: [1, 2, 3, 4, 5, 6],
            mean_error_ms: Some(-4.0), early: 10, late: 8, histogram: histogram(&[-30.0, 5.0, 7.0]), played_at: now(), duration_ms: 60_000, failed }
    }

    #[test]
    fn library_crud_and_duplicates() {
        let db = Db::open_memory().unwrap();
        assert_eq!(db.schema_version(), SCHEMA_VERSION);
        let id = db.add_song("aa", "/m/a.mp3", "A", "X", 180_000).unwrap();
        assert!(db.add_song("aa", "/m/copia.mp3", "A2", "", 1).is_err(), "huella duplicada debe rechazarse");
        db.set_status(id, Status::Ready, None, Some(128.0)).unwrap();
        let s = db.song(id).unwrap().unwrap();
        assert_eq!((s.status, s.bpm), (Status::Ready, Some(128.0)));
        assert_eq!(db.song_by_fingerprint("aa").unwrap().unwrap().title, "A");
        db.delete_song(id).unwrap();
        assert!(db.songs().unwrap().is_empty());
    }
    #[test]
    fn interrupted_analysis_is_recovered() {
        let db = Db::open_memory().unwrap();
        let id = db.add_song("bb", "/x", "B", "", 1).unwrap();
        assert_eq!(db.recover_interrupted().unwrap(), 1);
        let s = db.song(id).unwrap().unwrap();
        assert_eq!(s.status, Status::Failed);
    }
    #[test]
    fn best_score_ignores_failed_runs_and_other_difficulties() {
        let db = Db::open_memory().unwrap();
        db.add_score(&score("aa", "Hard", 800_000, false)).unwrap();
        db.add_score(&score("aa", "Hard", 950_000, true)).unwrap();
        db.add_score(&score("aa", "Easy", 990_000, false)).unwrap();
        db.add_score(&score("aa", "Hard", 850_000, false)).unwrap();
        assert_eq!(db.best("aa", "Hard").unwrap().unwrap().score, 850_000);
        assert!(db.best("zz", "Hard").unwrap().is_none());
    }
    #[test]
    fn empty_database_has_no_invented_stats() {
        let db = Db::open_memory().unwrap();
        let (n, ms, avg, combo) = db.totals().unwrap();
        assert_eq!((n, ms, avg, combo), (0, 0, None, None));
        assert!(db.recent(10).unwrap().is_empty());
        assert!(db.error_histogram().unwrap().iter().all(|c| *c == 0));
    }
    #[test]
    fn totals_and_histogram_aggregate() {
        let db = Db::open_memory().unwrap();
        db.add_score(&score("aa", "Easy", 1, false)).unwrap();
        db.add_score(&score("bb", "Easy", 2, false)).unwrap();
        let (n, ms, avg, _) = db.totals().unwrap();
        assert_eq!((n, ms), (2, 120_000));
        assert_eq!(avg, Some(90.0));
        assert_eq!(db.error_histogram().unwrap().iter().sum::<u32>(), 6);
        let r = db.recent(5).unwrap();
        assert_eq!(r[0].counts, [1, 2, 3, 4, 5, 6]);
    }
    #[test]
    fn histogram_bins() {
        let h = histogram(&[-200.0, 0.0, 9.9, 10.0, 500.0]);
        assert_eq!((h[0], h[15], h[16], h[29]), (1, 2, 1, 1));
    }
    #[test]
    fn corrupt_file_is_replaced_with_notice() {
        let dir = std::env::temp_dir().join(format!("nocturne-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("n.db");
        std::fs::write(&p, b"no soy una base de datos sqlite, solo basura de bytes largos para forzar el error de cabecera").unwrap();
        let (db, warn) = Db::open(&p);
        assert!(warn.is_some());
        assert!(db.songs().unwrap().is_empty());
        assert!(p.with_extension("db.corrupt").exists());
    }
}
