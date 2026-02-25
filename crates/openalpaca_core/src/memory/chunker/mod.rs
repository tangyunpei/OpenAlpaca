//! Markdown chunker for KB ingestion.

pub struct Chunk {
    pub content: String,
    pub source_file: String,
    pub section: Option<String>,
    pub index: usize,
}

/// Chunk markdown into segments at heading/paragraph boundaries.
/// Default `max_chunk_chars` is 500.
pub fn chunk_markdown(content: &str, source_file: &str, max_chunk_chars: usize) -> Vec<Chunk> {
    let max_chunk_chars = if max_chunk_chars == 0 {
        500
    } else {
        max_chunk_chars
    };
    let mut chunks = Vec::new();
    let mut index = 0;

    // Split on heading boundaries (## or ###)
    let sections = split_on_headings(content);

    for (heading, body) in sections {
        let section_text = if let Some(ref h) = heading {
            format!("{}\n{}", h, body)
        } else {
            body.to_string()
        };

        if section_text.trim().is_empty() {
            continue;
        }

        if section_text.len() <= max_chunk_chars {
            chunks.push(Chunk {
                content: section_text.trim().to_string(),
                source_file: source_file.to_string(),
                section: heading.clone(),
                index,
            });
            index += 1;
        } else {
            // Section too large — split on paragraphs
            for para in split_paragraphs(&section_text, max_chunk_chars) {
                if para.trim().is_empty() {
                    continue;
                }
                chunks.push(Chunk {
                    content: para.trim().to_string(),
                    source_file: source_file.to_string(),
                    section: heading.clone(),
                    index,
                });
                index += 1;
            }
        }
    }

    chunks
}

/// Split content on `## ` or `### ` headings, returning (heading, body) pairs.
fn split_on_headings(content: &str) -> Vec<(Option<String>, String)> {
    let mut sections: Vec<(Option<String>, String)> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = String::new();

    for line in content.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            // Flush previous section
            if !current_body.is_empty() || current_heading.is_some() {
                sections.push((current_heading.take(), std::mem::take(&mut current_body)));
            }
            current_heading = Some(line.to_string());
        } else {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }

    // Flush final section
    if !current_body.is_empty() || current_heading.is_some() {
        sections.push((current_heading, current_body));
    }

    sections
}

/// Split text into paragraph-sized chunks, further splitting on sentences if needed.
fn split_paragraphs(text: &str, max_chars: usize) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut result = Vec::new();

    for para in paragraphs {
        if para.len() <= max_chars {
            result.push(para.to_string());
        } else {
            // Split on sentence boundaries
            let sentences = split_sentences(para);
            if sentences.len() <= 1 {
                // No sentence boundaries — hard-split at max_chars
                let chars: Vec<char> = para.chars().collect();
                for chunk in chars.chunks(max_chars) {
                    let s: String = chunk.iter().collect();
                    if !s.is_empty() {
                        result.push(s);
                    }
                }
            } else {
                let mut current = String::new();
                for sentence in sentences {
                    if current.len() + sentence.len() + 1 > max_chars && !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(&sentence);
                }
                if !current.is_empty() {
                    result.push(current);
                }
            }
        }
    }

    result
}

/// Simple sentence splitter: splits on `. `, `! `, `? ` boundaries.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        current.push(chars[i]);
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && i + 1 < len
            && chars[i + 1] == ' '
        {
            sentences.push(std::mem::take(&mut current));
        }
        i += 1;
    }
    if !current.is_empty() {
        sentences.push(current);
    }

    sentences
}

#[cfg(test)]
mod tests;
