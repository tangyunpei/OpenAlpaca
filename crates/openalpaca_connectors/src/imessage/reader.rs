//! iMessage chat.db reader
//!
//! Reads incoming messages from the macOS Messages app SQLite database
//! (`~/Library/Messages/chat.db`) using a ROWID watermark to track
//! which messages have already been seen.

use rusqlite::{Connection, OpenFlags, params};

/// An attachment associated with an iMessage.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct IMessageAttachment {
    pub filename: String,
    pub mime_type: String,
    pub transfer_name: String,
    pub file_path: String,
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
        Ok(Self {
            conn,
            last_rowid: 0,
        })
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
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.ROWID, COALESCE(m.text, ''), h.id, c.chat_identifier
                 FROM message m
                 JOIN handle h ON m.handle_id = h.ROWID
                 JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
                 JOIN chat c ON c.ROWID = cmj.chat_id
                 WHERE m.ROWID > ?1
                   AND (?2 = 1 OR m.is_from_me = 0)
                   AND (m.text IS NOT NULL OR m.cache_has_attachments = 1)
                 ORDER BY m.ROWID ASC",
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let rows = stmt
            .query_map(
                params![self.last_rowid, if include_from_me { 1 } else { 0 }],
                |row| {
                    let rowid: i64 = row.get(0)?;
                    let text: String = row.get(1)?;
                    let sender: String = row.get(2)?;
                    let chat_id: String = row.get(3)?;
                    let is_group = chat_id.starts_with("chat");
                    Ok(IncomingMessage {
                        rowid,
                        text,
                        sender,
                        chat_id,
                        is_group,
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
                    return Err(format!("Failed to read message row: {}", e));
                }
            }
        }

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
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map([message_rowid], |row| {
            let filename: Option<String> = row.get(0)?;
            let mime_type: Option<String> = row.get(1)?;
            let transfer_name: Option<String> = row.get(2)?;
            let total_bytes: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            Ok((filename, mime_type, transfer_name, total_bytes))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
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
                filename: resolved.clone(),
                mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                transfer_name: transfer_name.unwrap_or_default(),
                file_path: resolved,
                total_bytes,
            });
        }
        attachments
    }
}
