//! What becomes of an attachment on its way to the model (U1–U3).
//!
//! **U1 — the model that will answer, not the model that was configured.**
//! The adaptation used to ask the registry about `LlmRouter::default_model()`.
//! On a local-only install that id is not in the registry at all (a disabled
//! provider's compiled defaults are pruned), so every capability question fell
//! through `unwrap_or(false)` and a vision model was handed
//! `[image attached — …]`. The one resolution is [`answering_model`], which
//! walks L3's substitution ladder through [`crate::runner::routed_model`] —
//! the same reader the context-window code uses (M5/S3). Nothing routable is
//! `None`, and a `None` means *leave every part exactly as it is*: the router's
//! own `NoRoutableModel` is the honest answer, not a placeholder claiming the
//! model cannot see.
//!
//! **U2 — every model reads text.** A model that takes no *native* document
//! part still reads the text the upload pipeline already extracted, so the
//! document travels as a labelled text block bounded by
//! `[upload.governance] max_extracted_text_chars` (the cap extraction itself
//! applies). A cut says so inside the block. The placeholder is left for a
//! document with no extracted text at all.
//!
//! **U3 — nothing is withheld silently.** Each site that withholds a part logs
//! it and, for the turn's own attachments, records a
//! [`SkippedAttachment`](crate::gateway::SkippedAttachment) that reaches
//! `HandleResult` → `GatewayResponse` → the SSE `done` frame.
//!
//! One policy, two call sites: the turn's own attachments
//! (`handler_attachments.rs`, which knows each file's id) and the parts that
//! come back out of history (`handler_helpers::adapt_parts_for_model`).

use openalpaca_llm::LlmRouter;

/// The model a turn will actually be answered by, and what it takes natively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnsweringModel {
    pub id: String,
    pub image: bool,
    pub audio: bool,
    pub document: bool,
    /// The model's context window in tokens — `None` when the registry holds
    /// no entry for it, or the entry's window is `0` (which must never reach
    /// compaction, which divides by it). Carried here so a caller that needs
    /// both the window and the media capabilities walks L3's ladder **once**
    /// and cannot budget against one model while adapting for another (A1).
    pub window: Option<usize>,
}

/// U1 — resolve the answering model once, the same way the window is resolved.
///
/// `pinned` is the id the loop will pass to the router (`LoopConfig.model`).
/// `None` when nothing at all is routable; callers must then leave the parts
/// untouched.
pub(crate) fn answering_model(router: &LlmRouter, pinned: Option<&str>) -> Option<AnsweringModel> {
    let id = crate::runner::routed_model(router, pinned)?;
    let registry = router.model_registry();
    Some(AnsweringModel {
        image: registry.supports_image(&id),
        audio: registry.supports_audio(&id),
        document: registry.supports_document(&id),
        window: registry
            .get_model_info(&id)
            .map(|info| info.context_window as usize)
            .filter(|w| *w > 0),
        id,
    })
}

/// What happens to one attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fate {
    /// Sent as its native part — the model takes this kind.
    Native,
    /// U2 — sent as a labelled text block. `cut_at` is `Some(total_chars)`
    /// when the block does not carry the whole text.
    AsText { cut_at: Option<usize> },
    /// Withheld: a placeholder took its place and the model never saw it.
    Withheld { reason: &'static str },
}

pub(crate) const REASON_NO_IMAGE: &str = "the answering model does not support image input";
pub(crate) const REASON_NO_AUDIO: &str = "the answering model does not support audio input";
pub(crate) const REASON_NO_DOCUMENT: &str =
    "the answering model does not support document input and no text was extracted from this file";

/// The placeholder texts. Unchanged wording — a model that reads one has
/// always been able to tell the user what happened.
pub(crate) const PLACEHOLDER_IMAGE: &str = "[image attached — model does not support vision]";
pub(crate) const PLACEHOLDER_AUDIO: &str = "[audio attached — model does not support audio input]";
pub(crate) const PLACEHOLDER_DOCUMENT: &str =
    "[document attached — model does not support document input]";

pub(crate) fn image_fate(model: &AnsweringModel) -> Fate {
    match model.image {
        true => Fate::Native,
        false => Fate::Withheld {
            reason: REASON_NO_IMAGE,
        },
    }
}

/// A2 — an `audio/*` attachment is judged as audio.
///
/// The turn's own attachments used to ask [`document_fate`] about every
/// non-image file, so an audio clip's fate hung on `supports_document`; only
/// the history-replay path (`ContentPart::Audio`) ever reached this. A model
/// that takes no audio still reads a transcript, so U2 applies here for the
/// same reason it applies to a document: `extracted_text` is `None` on the
/// replay path, which lands on the placeholder exactly as before.
pub(crate) fn audio_fate(
    model: &AnsweringModel,
    extracted_text: Option<&str>,
    max_chars: usize,
) -> Fate {
    if model.audio {
        return Fate::Native;
    }
    as_text_or_withheld(extracted_text, max_chars, REASON_NO_AUDIO)
}

/// U2 — a document the model cannot take natively becomes text when there is
/// text to send.
pub(crate) fn document_fate(
    model: &AnsweringModel,
    extracted_text: Option<&str>,
    max_chars: usize,
) -> Fate {
    if model.document {
        return Fate::Native;
    }
    as_text_or_withheld(extracted_text, max_chars, REASON_NO_DOCUMENT)
}

/// U2's shared tail: what the model cannot take natively travels as text when
/// the upload pipeline extracted some, and is withheld when it did not.
fn as_text_or_withheld(
    extracted_text: Option<&str>,
    max_chars: usize,
    reason: &'static str,
) -> Fate {
    match extracted_text {
        Some(text) if !text.trim().is_empty() => {
            let total = text.chars().count();
            Fate::AsText {
                cut_at: (total > max_chars).then_some(total),
            }
        }
        _ => Fate::Withheld { reason },
    }
}

/// U2 — the labelled block a document's extracted text travels in.
///
/// Bounded by `max_chars`; a cut is named inside the block so the model can
/// say so, and the caller logs it (U3).
pub(crate) fn document_as_text(
    filename: &str,
    mime_type: &str,
    text: &str,
    max_chars: usize,
) -> String {
    let total = text.chars().count();
    let body: String = match total > max_chars {
        true => text.chars().take(max_chars).collect(),
        false => text.to_string(),
    };
    let mut block = format!("Attached file: {filename} ({mime_type})\n```\n{body}\n```");
    if total > max_chars {
        block.push_str(&format!(
            "\n[truncated — {max_chars} of {total} characters shown]"
        ));
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(image: bool, audio: bool, document: bool) -> AnsweringModel {
        AnsweringModel {
            id: "m".to_string(),
            image,
            audio,
            document,
            window: Some(32_768),
        }
    }

    #[test]
    fn a_document_with_text_becomes_text_when_the_model_takes_no_native_document() {
        assert_eq!(
            document_fate(&model(false, false, false), Some("hello"), 100),
            Fate::AsText { cut_at: None }
        );
    }

    #[test]
    fn a_document_with_no_text_is_withheld() {
        assert_eq!(
            document_fate(&model(false, false, false), None, 100),
            Fate::Withheld {
                reason: REASON_NO_DOCUMENT
            }
        );
        assert_eq!(
            document_fate(&model(false, false, false), Some("   \n"), 100),
            Fate::Withheld {
                reason: REASON_NO_DOCUMENT
            }
        );
    }

    #[test]
    fn a_native_document_model_keeps_the_native_part() {
        assert_eq!(
            document_fate(&model(false, false, true), Some("hello"), 100),
            Fate::Native
        );
    }

    /// U2 — bounded, and the cut is in the block itself.
    #[test]
    fn an_over_long_document_is_cut_and_says_so() {
        let text = "x".repeat(50);
        assert_eq!(
            document_fate(&model(false, false, false), Some(&text), 10),
            Fate::AsText { cut_at: Some(50) }
        );
        let block = document_as_text("a.txt", "text/plain", &text, 10);
        assert!(block.contains("Attached file: a.txt (text/plain)"));
        assert!(block.contains("[truncated — 10 of 50 characters shown]"));
        assert!(!block.contains(&"x".repeat(11)));
    }

    /// A multi-byte document is cut on characters, never on bytes — a cut
    /// inside a UTF-8 sequence would panic on a byte slice.
    #[test]
    fn the_cut_counts_characters() {
        let text = "日本語テキスト".repeat(10);
        let block = document_as_text("a.txt", "text/plain", &text, 5);
        assert!(block.contains("日本語テキ"));
        assert!(block.contains("of 70 characters shown"));
    }

    #[test]
    fn an_image_survives_a_vision_model_and_is_withheld_from_the_rest() {
        assert_eq!(image_fate(&model(true, false, false)), Fate::Native);
        assert_eq!(
            image_fate(&model(false, false, false)),
            Fate::Withheld {
                reason: REASON_NO_IMAGE
            }
        );
        assert_eq!(
            audio_fate(&model(false, true, false), None, 100),
            Fate::Native
        );
    }

    /// **A2** — audio is judged by `supports_audio`, never by
    /// `supports_document`: a model that takes documents but no audio must
    /// still withhold the clip, and one that takes audio keeps it even though
    /// it takes no documents.
    #[test]
    fn audio_is_judged_as_audio_not_as_a_document() {
        assert_eq!(
            audio_fate(&model(false, false, true), None, 100),
            Fate::Withheld {
                reason: REASON_NO_AUDIO
            }
        );
        assert_eq!(
            audio_fate(&model(false, true, false), None, 100),
            Fate::Native
        );
    }

    /// U2 reaches audio too: a clip the model cannot hear still travels as its
    /// transcript when the upload pipeline extracted one.
    #[test]
    fn an_unhearable_clip_travels_as_its_transcript() {
        assert_eq!(
            audio_fate(&model(false, false, false), Some("hello there"), 100),
            Fate::AsText { cut_at: None }
        );
    }
}
