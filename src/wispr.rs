use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    env,
    error::Error,
    io,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const CLOCK_SLOP: f64 = 0.25;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
pub struct Capture {
    completed_rowid: Option<i64>,
    started: f64,
}

pub fn capture(started: SystemTime) -> Capture {
    let started = started
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        - CLOCK_SLOP;
    let completed_rowid = open()
        .ok()
        .and_then(|connection| checkpoint(&connection).ok());

    Capture {
        completed_rowid,
        started,
    }
}

pub fn transcript(capture: Capture) -> Result<String> {
    let connection = open()?;
    let deadline = Instant::now() + LOOKUP_TIMEOUT;

    loop {
        let transcript = match capture.completed_rowid {
            Some(rowid) => latest_after(&connection, rowid)?,
            None => latest_since(&connection, capture.started)?,
        };
        if let Some(transcript) = transcript {
            let transcript = transcript.trim();
            if !transcript.is_empty() {
                return Ok(transcript.to_owned());
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Wispr transcript did not become available").into());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn open() -> Result<Connection> {
    let connection = Connection::open_with_flags(
        database_path()?,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(50))?;
    Ok(connection)
}

fn checkpoint(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        r#"SELECT COALESCE(MAX(rowid), 0)
           FROM History
           WHERE app = 'com.cmuxterm.app'
             AND transcriptCommand = 'ptt'
             AND status = 'formatted'"#,
        [],
        |row| row.get(0),
    )
}

fn latest_after(connection: &Connection, rowid: i64) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            r#"SELECT COALESCE(
                   NULLIF(pastedText, ''),
                   NULLIF(formattedText, ''),
                   NULLIF(serverFinalizedText, ''),
                   NULLIF(asrText, '')
               )
               FROM History
               WHERE app = 'com.cmuxterm.app'
                 AND transcriptCommand = 'ptt'
                 AND status = 'formatted'
                 AND rowid > ?1
               ORDER BY rowid ASC
               LIMIT 1"#,
            [rowid],
            |row| row.get(0),
        )
        .optional()
}

fn latest_since(connection: &Connection, started: f64) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            r#"SELECT COALESCE(
                   NULLIF(pastedText, ''),
                   NULLIF(formattedText, ''),
                   NULLIF(serverFinalizedText, ''),
                   NULLIF(asrText, '')
               )
               FROM History
               WHERE app = 'com.cmuxterm.app'
                 AND transcriptCommand = 'ptt'
                 AND status = 'formatted'
                 AND (julianday(timestamp) - 2440587.5) * 86400.0 >= ?1
               ORDER BY timestamp ASC
               LIMIT 1"#,
            [started],
            |row| row.get(0),
        )
        .optional()
}

fn database_path() -> Result<PathBuf> {
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unavailable"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/Wispr Flow/flow.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::{checkpoint, latest_after, latest_since};
    use rusqlite::Connection;

    #[test]
    fn reads_transcript_completed_after_capture() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE History (
                       timestamp TEXT,
                       pastedText TEXT,
                       formattedText TEXT,
                       serverFinalizedText TEXT,
                       asrText TEXT,
                       app TEXT,
                       transcriptCommand TEXT,
                       status TEXT
                   );
                   INSERT INTO History VALUES
                       ('2026-08-26 20:00:00.000 +00:00', 'old', NULL, NULL, NULL, 'com.cmuxterm.app', 'ptt', 'formatted'),
                       ('2026-08-26 20:00:02.000 +00:00', 'Open Netflix.', NULL, NULL, NULL, 'com.cmuxterm.app', 'ptt', 'recording'),
                       ('2026-08-26 20:00:03.000 +00:00', 'wrong app', NULL, NULL, NULL, 'com.apple.Notes', 'ptt', 'formatted');"#,
            )
            .unwrap();

        let rowid = checkpoint(&connection).unwrap();
        connection
            .execute(
                "UPDATE History SET status = 'formatted' WHERE pastedText = 'Open Netflix.'",
                [],
            )
            .unwrap();

        assert_eq!(
            latest_after(&connection, rowid).unwrap().as_deref(),
            Some("Open Netflix.")
        );
    }

    #[test]
    fn falls_back_to_timestamp_when_capture_was_unavailable() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE History (
                       timestamp TEXT,
                       pastedText TEXT,
                       formattedText TEXT,
                       serverFinalizedText TEXT,
                       asrText TEXT,
                       app TEXT,
                       transcriptCommand TEXT,
                       status TEXT
                   );
                   INSERT INTO History VALUES
                       ('2026-08-26 20:00:00.000 +00:00', 'old', NULL, NULL, NULL, 'com.cmuxterm.app', 'ptt', 'formatted'),
                       ('2026-08-26 20:00:02.000 +00:00', 'Open Netflix.', NULL, NULL, NULL, 'com.cmuxterm.app', 'ptt', 'formatted');"#,
            )
            .unwrap();

        let started = 1_787_774_401.0;
        assert_eq!(
            latest_since(&connection, started).unwrap().as_deref(),
            Some("Open Netflix.")
        );
    }
}
