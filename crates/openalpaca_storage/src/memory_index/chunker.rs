//! Deterministic, heading- and paragraph-aware chunking of a note body
//! (design §4: "about 1–2 KiB, on heading/paragraph boundaries").
//!
//! The body is split into blocks — a heading line, a paragraph (a maximal run
//! of non-blank lines), or a fenced code block kept whole — and consecutive
//! blocks are packed into chunks of at most [`CHUNK_TARGET_BYTES`], cutting at
//! block boundaries. A heading starts a new chunk once the current one holds
//! at least half the target, so sections line up with chunks when they are of
//! any size. A block larger than [`CHUNK_MAX_BYTES`] is split at line
//! boundaries, and a single line larger than that at character boundaries, so
//! no chunk exceeds the maximum and none cuts a UTF-8 sequence.
//!
//! Every chunk carries its 1-based line span in the body and the text of
//! exactly those lines, so a citation's `line_range` names what the excerpt
//! shows. The chunks' spans cover every non-blank line of the body once.

/// The packing target (design §4: about 1–2 KiB).
pub const CHUNK_TARGET_BYTES: usize = 1536;
/// The hard bound: no chunk's text is larger than this.
pub const CHUNK_MAX_BYTES: usize = 2048;

/// One chunk of a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position within the note, from 0.
    pub ordinal: u32,
    /// The nearest heading at or above the chunk's first line, without the
    /// `#` markers.
    pub heading: Option<String>,
    /// 1-based, inclusive, relative to the body's first line.
    pub line_from: u32,
    pub line_to: u32,
    /// The lines `line_from..=line_to` joined with `\n`, no trailing newline.
    pub text: String,
}

/// A run of body lines the packer treats as one unit.
struct Block {
    /// 0-based line indexes, inclusive.
    first: usize,
    last: usize,
    /// The heading text when the block is a heading line.
    heading: Option<String>,
}

/// Splits `body` into chunks. An empty or blank body yields no chunks.
pub fn chunk_body(body: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = split_lines(body);
    let blocks = blocks_of(&lines);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current_heading: Option<String> = None;
    // The open chunk: its first block index, last line, and heading.
    let mut open: Option<(usize, usize, Option<String>)> = None;

    let byte_len = |first: usize, last: usize| -> usize {
        let text_bytes: usize = lines[first..=last].iter().map(|l| l.len()).sum();
        text_bytes + (last - first)
    };

    let flush = |open: &mut Option<(usize, usize, Option<String>)>, chunks: &mut Vec<Chunk>| {
        if let Some((first, last, heading)) = open.take() {
            push_bounded(&lines, first, last, heading, chunks);
        }
    };

    for block in &blocks {
        if let Some(text) = &block.heading {
            current_heading = Some(text.clone());
        }
        match open.as_mut() {
            None => {
                open = Some((block.first, block.last, current_heading.clone()));
            }
            Some((first, last, _)) => {
                let combined = byte_len(*first, block.last);
                let heading_cut =
                    block.heading.is_some() && byte_len(*first, *last) >= CHUNK_TARGET_BYTES / 2;
                if combined > CHUNK_TARGET_BYTES || heading_cut {
                    flush(&mut open, &mut chunks);
                    open = Some((block.first, block.last, current_heading.clone()));
                } else {
                    *last = block.last;
                }
            }
        }
    }
    flush(&mut open, &mut chunks);

    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.ordinal = i as u32;
    }
    chunks
}

/// Splits on `\n`, dropping the empty segment a trailing newline leaves.
fn split_lines(body: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = body.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Groups lines into blocks: headings, paragraphs, and fenced code kept whole.
fn blocks_of(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(text) = heading_text(line) {
            blocks.push(Block {
                first: i,
                last: i,
                heading: Some(text),
            });
            i += 1;
            continue;
        }
        if is_fence(line) {
            let start = i;
            let mut j = i + 1;
            while j < lines.len() && !is_fence(lines[j]) {
                j += 1;
            }
            let end = if j < lines.len() { j } else { lines.len() - 1 };
            blocks.push(Block {
                first: start,
                last: end,
                heading: None,
            });
            i = end + 1;
            continue;
        }
        let start = i;
        while i < lines.len()
            && !lines[i].trim().is_empty()
            && heading_text(lines[i]).is_none()
            && !is_fence(lines[i])
        {
            i += 1;
        }
        blocks.push(Block {
            first: start,
            last: i - 1,
            heading: None,
        });
    }
    blocks
}

/// `# Title` … `###### Title` → `Title`; anything else is not a heading.
fn heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some(text.to_string())
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Emits the lines `first..=last` as one chunk when they fit
/// [`CHUNK_MAX_BYTES`], else as several: first by line boundaries, then a
/// single over-long line by character boundaries.
fn push_bounded(
    lines: &[&str],
    first: usize,
    last: usize,
    heading: Option<String>,
    chunks: &mut Vec<Chunk>,
) {
    let mut start = first;
    while start <= last {
        // Extend `end` while the span fits.
        let mut end = start;
        let mut bytes = lines[start].len();
        if bytes > CHUNK_MAX_BYTES {
            // One line larger than the bound: cut it on char boundaries.
            for piece in split_line(lines[start]) {
                chunks.push(Chunk {
                    ordinal: 0,
                    heading: heading.clone(),
                    line_from: start as u32 + 1,
                    line_to: start as u32 + 1,
                    text: piece.to_string(),
                });
            }
            start += 1;
            continue;
        }
        while end < last {
            let next = bytes + 1 + lines[end + 1].len();
            if next > CHUNK_MAX_BYTES {
                break;
            }
            bytes = next;
            end += 1;
        }
        chunks.push(Chunk {
            ordinal: 0,
            heading: heading.clone(),
            line_from: start as u32 + 1,
            line_to: end as u32 + 1,
            text: lines[start..=end].join("\n"),
        });
        start = end + 1;
    }
}

/// Cuts one line into pieces of at most [`CHUNK_MAX_BYTES`] bytes, each ending
/// on a character boundary; the pieces concatenate to the line.
fn split_line(line: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = line;
    while rest.len() > CHUNK_MAX_BYTES {
        let cut = rest.floor_char_boundary(CHUNK_MAX_BYTES);
        let (head, tail) = rest.split_at(cut);
        pieces.push(head);
        rest = tail;
    }
    pieces.push(rest);
    pieces
}
