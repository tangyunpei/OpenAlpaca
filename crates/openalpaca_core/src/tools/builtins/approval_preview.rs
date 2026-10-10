//! What an approval card says about where a call acts (storage design §12,
//! R34): the daemon's own text, shown above the call's arguments
//! (`BuiltInTool::confirmation_preview`). A folder prints as it is: a
//! project's root is the owner's own path, and a scratch folder or a store is
//! OpenAlpaca's. A path the model wrote is third-party text and goes through
//! `utils::text`, with the notice under it (CLAUDE.md, Gotchas).

use std::path::Path;

use crate::tools::working_folder::WorkingFolder;
use crate::utils::text::{escape_hidden_line, hidden_notice};

/// The card of a command (`shell_execute`): the folder it starts in, and
/// that the folder does not confine it (ST-14).
pub(super) fn command_card(folder: &WorkingFolder) -> String {
    match folder {
        WorkingFolder::Project { root } => format!(
            "Runs in the project folder {}. The command itself can reach files outside it.",
            root.display()
        ),
        WorkingFolder::Scratch { dir, .. } => format!(
            "Runs in this conversation's scratch folder {}. The command itself can reach files \
             outside it.",
            dir.display()
        ),
    }
}

/// Where a write lands, for its card (R34).
pub(super) enum WriteLanding<'a> {
    /// A real file in the project (ST-10, ST-12). `replaces`: a file is there
    /// now, and its content is kept in the conversation's log first (ST-17).
    Project { root: &'a Path, replaces: bool },
    /// A by-product in `store`, the artifact store directory
    /// (`store_root(scope)?.join("artifacts")` — creates nothing).
    ByProduct { store: &'a Path },
}

/// The card of a file write: the file, where it lands, and — when the path
/// the model wrote holds a hidden character — the notice on its own line.
pub(super) fn write_card(path: &str, landing: WriteLanding<'_>) -> String {
    let shown = escape_hidden_line(path);
    let line = match landing {
        WriteLanding::ByProduct { store } => format!(
            "Saves {} as a by-product in {}, OpenAlpaca's artifact store — not in a folder of \
             yours.",
            shown.text,
            store.display()
        ),
        WriteLanding::Project {
            root,
            replaces: false,
        } => format!(
            "Writes {} in the project folder {}.",
            shown.text,
            root.display()
        ),
        WriteLanding::Project {
            root,
            replaces: true,
        } => format!(
            "Replaces {} in the project folder {}; its current content is kept in this \
             conversation's log.",
            shown.text,
            root.display()
        ),
    };
    match hidden_notice(shown.hidden) {
        Some(notice) => format!("{line}\n{notice}"),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

    #[test]
    fn a_command_card_names_the_project_folder_and_that_it_does_not_confine() {
        let folder = WorkingFolder::Project {
            root: PathBuf::from("/Users/me/acme"),
        };
        assert_eq!(
            command_card(&folder),
            "Runs in the project folder /Users/me/acme. The command itself can reach files \
             outside it."
        );
    }

    #[test]
    fn a_command_card_with_no_project_names_the_conversations_scratch_folder() {
        let folder = WorkingFolder::Scratch {
            dir: PathBuf::from("/Users/me/.openalpaca/scratch/conv-1"),
            conversation_id: "conv-1".to_string(),
        };
        assert_eq!(
            command_card(&folder),
            "Runs in this conversation's scratch folder /Users/me/.openalpaca/scratch/conv-1. \
             The command itself can reach files outside it."
        );
    }

    #[test]
    fn a_by_product_write_card_names_the_artifact_store() {
        assert_eq!(
            write_card(
                "reports/pasta-night.md",
                WriteLanding::ByProduct {
                    store: Path::new("/Users/me/.openalpaca/artifacts")
                }
            ),
            "Saves reports/pasta-night.md as a by-product in /Users/me/.openalpaca/artifacts, \
             OpenAlpaca's artifact store — not in a folder of yours."
        );
    }

    #[test]
    fn a_project_write_card_names_the_folder_and_whether_a_file_is_replaced() {
        let root = Path::new("/Users/me/dev/app");
        assert_eq!(
            write_card(
                "src/main.rs",
                WriteLanding::Project {
                    root,
                    replaces: false
                }
            ),
            "Writes src/main.rs in the project folder /Users/me/dev/app."
        );
        assert_eq!(
            write_card(
                "src/main.rs",
                WriteLanding::Project {
                    root,
                    replaces: true
                }
            ),
            "Replaces src/main.rs in the project folder /Users/me/dev/app; its current content is \
             kept in this conversation's log."
        );
    }

    /// The path is the model's: an override and a zero-width space are shown
    /// as `<U+XXXX>` and counted on a line of their own.
    #[test]
    fn a_hidden_character_in_the_path_is_shown_escaped_with_the_notice_under_the_card() {
        let card = write_card(
            "notes/\u{202e}dm\u{200b}.txt",
            WriteLanding::ByProduct {
                store: Path::new("/s/artifacts"),
            },
        );

        assert_eq!(
            card,
            "Saves notes/<U+202E>dm<U+200B>.txt as a by-product in /s/artifacts, OpenAlpaca's \
             artifact store — not in a folder of yours.\n\
             Note: this request contains 2 hidden characters, shown as <U+…>."
        );
        assert!(!card.contains('\u{202e}') && !card.contains('\u{200b}'), "{card:?}");
    }
}
