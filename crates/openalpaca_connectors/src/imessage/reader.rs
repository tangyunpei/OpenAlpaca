//! iMessage chat.db reader
//!
//! Reads incoming messages from the macOS Messages app SQLite database
//! (`~/Library/Messages/chat.db`) using a ROWID watermark to track
//! which messages have already been seen.

use rusqlite::{Connection, OpenFlags, params};
use tracing::warn;

/// An attachment associated with an iMessage.
#[derive(Debug, Clone)]
pub struct IMessageAttachment {
    /// The original filename from chat.db (before `~` expansion).
    #[allow(dead_code)]
    pub filename: String,
    pub mime_type: String,
    pub transfer_name: String,
    pub file_path: String,
    #[allow(dead_code)]
    pub total_bytes: i64,
}

/// An incoming iMessage read from chat.db.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// The ROWID from the message table.
    pub rowid: i64,
    /// The message text content (may be empty for attachment-only messages).
    pub text: String,
    /// The sender identifier (phone number or email) from the handle table.
    pub sender: String,
    /// The chat identifier from the chat table.
    pub chat_id: String,
    /// Whether this message is from a group chat (chat_identifier starts with "chat").
    pub is_group: bool,
    /// Whether this message was sent by the local Mac user (is_from_me = 1 in chat.db).
    pub is_from_me: bool,
    /// The iMessage account that sent this message (e.g. "p:+1234567890" or "e:user@icloud.com").
    /// Used to distinguish which device/handle sent it when multiple devices share the same Apple ID.
    pub account: String,
    /// Attachments associated with this message.
    pub attachments: Vec<IMessageAttachment>,
}

/// Reads new messages from the macOS Messages chat.db SQLite database.
///
/// Uses a ROWID watermark so that only messages newer than the last poll
/// are returned. The database is opened read-only to avoid interfering
/// with the Messages app. A single persistent connection is reused across
/// polls to avoid the overhead of opening/closing on every cycle.
pub struct ChatDbReader {
    conn: Connection,
    last_rowid: i64,
    /// Whether the `message` table has a `date_deleted` column (added in newer macOS versions).
    has_date_deleted: bool,
}

impl ChatDbReader {
    /// Create a new reader targeting the given chat.db path.
    ///
    /// Opens a persistent read-only connection. The watermark starts at 0;
    /// call [`initialize_watermark`] before polling to skip historical messages.
    pub fn new(db_path: &str) -> Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(db_path, flags)
            .map_err(|e| format!("Failed to open chat.db at {}: {}", db_path, e))?;
        // Set a busy timeout in case the Messages app holds a lock briefly
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| format!("Failed to set busy_timeout: {}", e))?;
        let has_date_deleted = Self::probe_date_deleted(&conn);
        Ok(Self {
            conn,
            last_rowid: 0,
            has_date_deleted,
        })
    }

    /// Check whether the `message` table has a `date_deleted` column.
    /// Older macOS versions (pre-Catalina) lack this column.
    fn probe_date_deleted(conn: &Connection) -> bool {
        conn.prepare("SELECT date_deleted FROM message LIMIT 0")
            .is_ok()
    }

    /// Set the watermark to the current maximum ROWID in the message table.
    ///
    /// Call this once at startup so that the connector does not replay
    /// every historical message.
    pub fn initialize_watermark(&mut self) -> Result<(), String> {
        let max_rowid: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(ROWID), 0) FROM message", [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("Failed to query max ROWID: {}", e))?;
        self.last_rowid = max_rowid;
        Ok(())
    }

    /// Restore the watermark to a previously persisted value.
    pub fn set_watermark(&mut self, rowid: i64) {
        self.last_rowid = rowid;
    }

    /// Return the current watermark value for persistence.
    pub fn watermark(&self) -> i64 {
        self.last_rowid
    }

    /// Poll for new messages since the last watermark.
    ///
    /// By default, only messages where `is_from_me = 0` are returned (i.e.,
    /// messages sent by other people). If `include_from_me` is true, both
    /// incoming and outgoing messages are considered. After a successful poll
    /// the watermark is
    /// advanced to the highest ROWID seen.
    pub fn poll_new_messages(
        &mut self,
        include_from_me: bool,
    ) -> Result<Vec<IncomingMessage>, String> {
        let deleted_filter = if self.has_date_deleted {
            "AND (m.date_deleted IS NULL OR m.date_deleted = 0)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT m.ROWID, COALESCE(m.text, ''), COALESCE(h.id, ''), c.chat_identifier, m.is_from_me, COALESCE(m.account, '')
                 FROM message m
                 LEFT JOIN handle h ON m.handle_id = h.ROWID
                 JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
                 JOIN chat c ON c.ROWID = cmj.chat_id
                 WHERE m.ROWID > ?1
                   AND m.service = 'iMessage'
                   {deleted_filter}
                   AND (?2 = 1 OR m.is_from_me = 0)
                   AND (m.text IS NOT NULL OR m.cache_has_attachments = 1)
                 ORDER BY m.ROWID ASC, cmj.chat_id ASC",
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let rows = stmt
            .query_map(
                params![self.last_rowid, if include_from_me { 1 } else { 0 }],
                |row| {
                    let rowid: i64 = row.get(0)?;
                    let text: String = row.get(1)?;
                    let sender: String = row.get(2)?;
                    let chat_id: String = row.get(3)?;
                    let is_from_me_int: i64 = row.get(4)?;
                    let account: String = row.get(5)?;
                    // Apple prefixes group chat identifiers with "chat" (e.g., "chat123456789"),
                    // while DMs use "iMessage;-;+15551234567" or "iMessage;-;user@example.com".
                    let is_group = chat_id.starts_with("chat");
                    Ok(IncomingMessage {
                        rowid,
                        text,
                        sender,
                        chat_id,
                        is_group,
                        is_from_me: is_from_me_int != 0,
                        account,
                        attachments: Vec::new(), // populated below
                    })
                },
            )
            .map_err(|e| format!("Failed to query messages: {}", e))?;

        let mut messages = Vec::new();
        for row in rows {
            match row {
                Ok(mut msg) => {
                    if msg.rowid > self.last_rowid {
                        self.last_rowid = msg.rowid;
                    }
                    // Fetch attachments for this message
                    msg.attachments = self.get_attachments_for_message(msg.rowid);
                    messages.push(msg);
                }
                Err(e) => {
                    warn!("Failed to read message row, skipping: {e}");
                    continue;
                }
            }
        }

        // Deduplicate by ROWID: a message linked to multiple chats via
        // chat_message_join produces duplicate rows. Secondary sort by
        // chat_id ASC ensures deterministic selection (smallest chat_id).
        messages.dedup_by_key(|m| m.rowid);

        Ok(messages)
    }

    /// Fetch attachments for a specific message ROWID from chat.db.
    fn get_attachments_for_message(&self, message_rowid: i64) -> Vec<IMessageAttachment> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
        let mut stmt = match self.conn.prepare(
            "SELECT a.filename, a.mime_type, a.transfer_name, a.total_bytes
             FROM attachment a
             JOIN message_attachment_join maj ON a.ROWID = maj.attachment_id
             WHERE maj.message_id = ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to prepare attachment query for message {message_rowid}: {e}");
                return Vec::new();
            }
        };

        let rows = match stmt.query_map([message_rowid], |row| {
            let filename: Option<String> = row.get(0)?;
            let mime_type: Option<String> = row.get(1)?;
            let transfer_name: Option<String> = row.get(2)?;
            let total_bytes: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            Ok((filename, mime_type, transfer_name, total_bytes))
        }) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to query attachments for message {message_rowid}: {e}");
                return Vec::new();
            }
        };

        let mut attachments = Vec::new();
        for (filename, mime_type, transfer_name, total_bytes) in rows.flatten() {
            // Resolve ~ to $HOME in the filename path
            let raw_path = filename.unwrap_or_default();
            let resolved = raw_path.replacen('~', &home, 1);

            // Skip files that don't exist on disk
            if resolved.is_empty() || !std::path::Path::new(&resolved).exists() {
                continue;
            }

            attachments.push(IMessageAttachment {
                filename: raw_path,
                mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                transfer_name: transfer_name.unwrap_or_default(),
                file_path: resolved,
                total_bytes,
            });
        }
        attachments
    }

    /// Test-only constructor accepting a pre-built connection.
    #[cfg(test)]
    fn from_connection(conn: Connection) -> Self {
        let has_date_deleted = Self::probe_date_deleted(&conn);
        Self {
            conn,
            last_rowid: 0,
            has_date_deleted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite database with the minimal chat.db schema
    /// needed by `ChatDbReader`.
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT);
             CREATE TABLE message (
                 ROWID INTEGER PRIMARY KEY,
                 text TEXT,
                 handle_id INTEGER,
                 is_from_me INTEGER DEFAULT 0,
                 account TEXT DEFAULT '',
                 cache_has_attachments INTEGER DEFAULT 0,
                 service TEXT DEFAULT 'iMessage',
                 date_deleted INTEGER DEFAULT 0
             );
             CREATE TABLE chat_message_join (message_id INTEGER, chat_id INTEGER);
             CREATE TABLE attachment (
                 ROWID INTEGER PRIMARY KEY,
                 filename TEXT,
                 mime_type TEXT,
                 transfer_name TEXT,
                 total_bytes INTEGER DEFAULT 0
             );
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);",
        )
        .unwrap();
        conn
    }

    /// Insert a handle and return its ROWID.
    fn insert_handle(conn: &Connection, id: &str) -> i64 {
        conn.execute("INSERT INTO handle (id) VALUES (?1)", [id])
            .unwrap();
        conn.last_insert_rowid()
    }

    /// Insert a chat and return its ROWID.
    fn insert_chat(conn: &Connection, chat_identifier: &str) -> i64 {
        conn.execute(
            "INSERT INTO chat (chat_identifier) VALUES (?1)",
            [chat_identifier],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Insert a message and link it to a chat. Returns the message ROWID.
    fn insert_message(
        conn: &Connection,
        text: &str,
        handle_rowid: i64,
        chat_rowid: i64,
        service: &str,
        date_deleted: i64,
    ) -> i64 {
        conn.execute(
            "INSERT INTO message (text, handle_id, is_from_me, account, cache_has_attachments, service, date_deleted)
             VALUES (?1, ?2, 0, '', 0, ?3, ?4)",
            rusqlite::params![text, handle_rowid, service, date_deleted],
        )
        .unwrap();
        let msg_rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (?1, ?2)",
            [msg_rowid, chat_rowid],
        )
        .unwrap();
        msg_rowid
    }

    #[test]
    fn test_filters_sms_messages() {
        let conn = create_test_db();
        let handle = insert_handle(&conn, "+15551234567");
        let chat = insert_chat(&conn, "iMessage;-;+15551234567");

        // iMessage message — should be returned
        insert_message(&conn, "hello via imessage", handle, chat, "iMessage", 0);
        // SMS message — should be filtered out
        insert_message(&conn, "hello via sms", handle, chat, "SMS", 0);

        let mut reader = ChatDbReader::from_connection(conn);
        let messages = reader.poll_new_messages(false).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "hello via imessage");
    }

    #[test]
    fn test_filters_deleted_messages() {
        let conn = create_test_db();
        let handle = insert_handle(&conn, "+15551234567");
        let chat = insert_chat(&conn, "iMessage;-;+15551234567");

        // Normal message
        insert_message(&conn, "visible", handle, chat, "iMessage", 0);
        // Deleted message (date_deleted != 0)
        insert_message(&conn, "deleted", handle, chat, "iMessage", 695000000);

        let mut reader = ChatDbReader::from_connection(conn);
        let messages = reader.poll_new_messages(false).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "visible");
    }

    /// Create an in-memory database WITHOUT the `date_deleted` column,
    /// simulating older macOS chat.db schemas.
    fn create_test_db_no_date_deleted() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT);
             CREATE TABLE message (
                 ROWID INTEGER PRIMARY KEY,
                 text TEXT,
                 handle_id INTEGER,
                 is_from_me INTEGER DEFAULT 0,
                 account TEXT DEFAULT '',
                 cache_has_attachments INTEGER DEFAULT 0,
                 service TEXT DEFAULT 'iMessage'
             );
             CREATE TABLE chat_message_join (message_id INTEGER, chat_id INTEGER);
             CREATE TABLE attachment (
                 ROWID INTEGER PRIMARY KEY,
                 filename TEXT,
                 mime_type TEXT,
                 transfer_name TEXT,
                 total_bytes INTEGER DEFAULT 0
             );
             CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_poll_without_date_deleted_column() {
        let conn = create_test_db_no_date_deleted();
        let handle = insert_handle(&conn, "+15551234567");
        let chat = insert_chat(&conn, "iMessage;-;+15551234567");

        conn.execute(
            "INSERT INTO message (text, handle_id, is_from_me, account, cache_has_attachments, service)
             VALUES (?1, ?2, 0, '', 0, 'iMessage')",
            rusqlite::params!["hello old macos", handle],
        )
        .unwrap();
        let msg_rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (?1, ?2)",
            [msg_rowid, chat],
        )
        .unwrap();

        let mut reader = ChatDbReader::from_connection(conn);
        assert!(!reader.has_date_deleted);
        let messages = reader.poll_new_messages(false).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "hello old macos");
    }

    #[test]
    fn test_dedup_by_rowid() {
        let conn = create_test_db();
        let handle = insert_handle(&conn, "+15551234567");
        let chat1 = insert_chat(&conn, "iMessage;-;+15551234567");
        let chat2 = insert_chat(&conn, "chat999999");

        // Insert one message linked to TWO chats (simulates shared/forwarded)
        conn.execute(
            "INSERT INTO message (text, handle_id, is_from_me, account, cache_has_attachments, service, date_deleted)
             VALUES ('shared msg', ?1, 0, '', 0, 'iMessage', 0)",
            [handle],
        )
        .unwrap();
        let msg_rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (?1, ?2)",
            [msg_rowid, chat1],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_message_join (message_id, chat_id) VALUES (?1, ?2)",
            [msg_rowid, chat2],
        )
        .unwrap();

        let mut reader = ChatDbReader::from_connection(conn);
        let messages = reader.poll_new_messages(false).unwrap();
        // Should be deduplicated to a single message
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "shared msg");
    }
}
