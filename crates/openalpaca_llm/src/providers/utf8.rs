//! UTF-8-safe decoding for streamed byte chunks.
//!
//! Network chunk boundaries follow transport framing, not character boundaries,
//! so a multi-byte UTF-8 character can be split across two chunks. Decoding each
//! chunk independently with `from_utf8_lossy` turns those split characters into
//! `U+FFFD` replacement characters — silent corruption, common for CJK output.
//! This adapter carries an incomplete trailing sequence into the next chunk.

use futures_util::{Stream, StreamExt};

/// Wrap a byte stream, yielding UTF-8 strings. Incomplete trailing multi-byte
/// sequences are held back and prepended to the following chunk, so a character
/// split across chunk boundaries is never corrupted.
pub(crate) fn utf8_chunks<E>(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<String, E>> + Send
where
    E: Send + 'static,
{
    futures_util::stream::unfold(
        (Box::pin(byte_stream), Vec::<u8>::new()),
        |(mut stream, mut pending)| async move {
            loop {
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        pending.extend_from_slice(&bytes);
                        let (text, keep) = split_valid_utf8(&pending);
                        pending = keep;
                        if text.is_empty() {
                            // Only an incomplete character so far — get more bytes.
                            continue;
                        }
                        return Some((Ok(text), (stream, pending)));
                    }
                    Some(Err(e)) => return Some((Err(e), (stream, pending))),
                    None => {
                        if pending.is_empty() {
                            return None;
                        }
                        // Stream ended mid-character: flush the remainder lossily.
                        let text = String::from_utf8_lossy(&pending).into_owned();
                        pending.clear();
                        return Some((Ok(text), (stream, pending)));
                    }
                }
            }
        },
    )
}

/// Split a byte buffer into (valid-UTF-8 prefix, incomplete trailing bytes).
/// Genuine invalid bytes in the middle are replaced with `U+FFFD`; only a
/// truncated trailing sequence is carried forward.
fn split_valid_utf8(buf: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(buf) {
        Ok(s) => (s.to_string(), Vec::new()),
        Err(e) => {
            let valid_up_to = e.valid_up_to();
            // Safe: bytes [..valid_up_to] are valid UTF-8 by definition.
            let mut text = std::str::from_utf8(&buf[..valid_up_to]).unwrap().to_string();
            match e.error_len() {
                // Incomplete sequence at the end — carry it to the next chunk.
                None => (text, buf[valid_up_to..].to_vec()),
                // A genuine invalid sequence mid-buffer — replace and continue.
                Some(len) => {
                    text.push('\u{FFFD}');
                    let (rest_text, rest_keep) = split_valid_utf8(&buf[valid_up_to + len..]);
                    text.push_str(&rest_text);
                    (text, rest_keep)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    async fn collect(chunks: Vec<&'static [u8]>) -> String {
        let s = stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, std::convert::Infallible>(bytes::Bytes::from_static(c))),
        );
        let mut out = String::new();
        let mut decoded = Box::pin(utf8_chunks(s));
        while let Some(Ok(part)) = decoded.next().await {
            out.push_str(&part);
        }
        out
    }

    #[tokio::test]
    async fn char_split_across_chunks_is_not_corrupted() {
        // "你好" is E4 BD A0 E5 A5 BD; split mid-character.
        let out = collect(vec![&[0xE4, 0xBD], &[0xA0, 0xE5, 0xA5, 0xBD]]).await;
        assert_eq!(out, "你好");
    }

    #[tokio::test]
    async fn ascii_and_multibyte_mix() {
        let out = collect(vec![b"hi ", &[0xE4, 0xBD, 0xA0], b"!"]).await;
        assert_eq!(out, "hi 你!");
    }

    #[tokio::test]
    async fn trailing_incomplete_flushed_lossily() {
        let out = collect(vec![b"ok", &[0xE4, 0xBD]]).await;
        assert!(out.starts_with("ok"));
    }
}
