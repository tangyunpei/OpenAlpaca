//! Small SQL value helpers shared by repositories.

use chrono::{DateTime, NaiveDateTime, Utc};

/// Escape a literal substring for SQLite LIKE with an explicit backslash escape.
///
/// Both halves are required: every pattern built from this must be matched with
/// `ESCAPE '\'`, or the escapes are read as literal backslashes and the escaped
/// metacharacters stay wildcards — a search for `100%` then matches every row.
pub(crate) fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub(crate) fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|datetime| datetime.and_utc())
}

pub(crate) fn parse_datetime_or_now(value: &str) -> DateTime<Utc> {
    parse_datetime(value).unwrap_or_else(Utc::now)
}
