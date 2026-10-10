//! What an approval card says about where a call acts (storage design §12,
//! R34): the daemon's own text, shown above the call's arguments
//! (`BuiltInTool::confirmation_preview`). A folder prints as it is: a
//! project's root is the owner's own path, and a scratch folder or a store is
//! OpenAlpaca's. A path the model wrote is third-party text and goes through
//! `utils::text`, with the notice under it (CLAUDE.md, Gotchas).

use std::path::Path;

use crate::tools::working_folder::WorkingFolder;
use crate::utils::text::{cut_notice, escape_hidden_line, escape_hidden_within, hidden_notice};

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
    /// `link_to`: the file the write changes, relative to `root`, when the
    /// path reaches it through a link inside the project (R-T31).
    Project {
        root: &'a Path,
        replaces: bool,
        link_to: Option<&'a str>,
    },
    /// A by-product in `store`, the artifact store directory
    /// (`store_root(scope)?.join("artifacts")` — creates nothing).
    ByProduct { store: &'a Path },
}

/// The card of a file write: the file, where it lands, and — when the path
/// the model wrote, or the file a link in the project leads it to, holds a
/// hidden character — the notice on its own line, counting both.
pub(super) fn write_card(path: &str, landing: WriteLanding<'_>) -> String {
    let shown = escape_hidden_line(path);
    let mut hidden = shown.hidden;
    let line = match landing {
        WriteLanding::ByProduct { store } => format!(
            "Saves {} as a by-product in {}, OpenAlpaca's artifact store — not in a folder of \
             yours.",
            shown.text,
            store.display()
        ),
        WriteLanding::Project {
            root,
            replaces,
            link_to,
        } => {
            let file = named_file(&shown.text, link_to, &mut hidden);
            if replaces {
                format!(
                    "Replaces {file} in the project folder {}; its current content is kept in \
                     this conversation's log.",
                    root.display()
                )
            } else {
                format!("Writes {file} in the project folder {}.", root.display())
            }
        }
    };
    match hidden_notice(hidden) {
        Some(notice) => format!("{line}\n{notice}"),
        None => line,
    }
}

/// R49's budget for each of `old_string` and `new_string` on `file_edit`'s
/// card, in bytes of escaped text: two strings and the frame stay a card a
/// person reads at a glance (~25 lines each), and the full arguments are
/// drawn below the preview anyway.
pub(super) const EDIT_PREVIEW_BYTES: usize = 1_000;

/// `file_edit`'s card (R49): the file — and, when the path reaches it through
/// a link inside the project, the file it leads to (`link_to`, relative to
/// `root`; R-T31) — the project folder and the line of the first occurrence,
/// then both strings — each escaped (`escape_hidden_within`, which keeps
/// `\n` and `\t` as their layout) and cut to [`EDIT_PREVIEW_BYTES`] with
/// the cut notice — then one notice counting every hidden character of the
/// path, the linked file and both strings. The folder prints as it is: the
/// owner's own path (interfaces §1.3).
pub(super) fn edit_card(
    path: &str,
    link_to: Option<&str>,
    root: &Path,
    first_line: usize,
    replacements: usize,
    old_string: &str,
    new_string: &str,
) -> String {
    let shown_path = escape_hidden_line(path);
    let mut hidden = shown_path.hidden;
    let file = named_file(&shown_path.text, link_to, &mut hidden);
    let old = escape_hidden_within(old_string, EDIT_PREVIEW_BYTES);
    let new = escape_hidden_within(new_string, EDIT_PREVIEW_BYTES);
    let mut lines = vec![format!(
        "Edits {file} in the project folder {}: {replacements} replacement{}, the first at line \
         {first_line}. Its current content is kept in this conversation's log.",
        root.display(),
        if replacements == 1 { "" } else { "s" }
    )];
    lines.push("Replace:".to_string());
    lines.push(old.text);
    lines.extend(cut_notice(old.left_out));
    lines.push("With:".to_string());
    lines.push(new.text);
    lines.extend(cut_notice(new.left_out));
    lines.extend(hidden_notice(hidden + old.hidden + new.hidden));
    lines.join("\n")
}

/// The file a write or edit card names: the path as the model wrote it
/// (`shown`, escaped already), and — when that path reaches the file through
/// a link inside the project — `(a link to {file})`, the file escaped and
/// its hidden characters added to `hidden` (R-T31).
fn named_file(shown: &str, link_to: Option<&str>, hidden: &mut usize) -> String {
    match link_to {
        Some(target) => {
            let target = escape_hidden_line(target);
            *hidden += target.hidden;
            format!("{shown} (a link to {})", target.text)
        }
        None => shown.to_string(),
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
                    replaces: false,
                    link_to: None,
                }
            ),
            "Writes src/main.rs in the project folder /Users/me/dev/app."
        );
        assert_eq!(
            write_card(
                "src/main.rs",
                WriteLanding::Project {
                    root,
                    replaces: true,
                    link_to: None,
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

#[cfg(test)]
mod edit_card_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_edit_card_names_the_file_the_folder_and_the_line_and_shows_both_strings() {
        let card = edit_card(
            "src/lib.rs",
            None,
            Path::new("/work/alpaca"),
            2,
            1,
            "    3\n}",
            "    5\n}",
        );
        assert_eq!(
            card,
            "Edits src/lib.rs in the project folder /work/alpaca: 1 replacement, the first at \
             line 2. Its current content is kept in this conversation's log.\n\
             Replace:\n    3\n}\nWith:\n    5\n}"
        );
    }

    #[test]
    fn several_replacements_read_in_the_plural() {
        let card = edit_card("a.txt", None, Path::new("/p"), 4, 3, "a", "b");
        assert!(
            card.starts_with(
                "Edits a.txt in the project folder /p: 3 replacements, the first at line 4."
            ),
            "{card}"
        );
    }

    /// R49 at the text layer: a U+202E in either string, and a U+200B in the
    /// path, are shown escaped, and one notice counts all three.
    #[test]
    fn hidden_characters_in_the_path_and_both_strings_are_escaped_under_one_notice() {
        let card = edit_card(
            "src/a\u{200B}.rs",
            None,
            Path::new("/p"),
            1,
            1,
            "let a = 1;\u{202E}",
            "let a = 2;\u{202E}",
        );
        assert!(
            !card.contains('\u{202E}') && !card.contains('\u{200B}'),
            "{card}"
        );
        let lines: Vec<&str> = card.lines().collect();
        assert!(
            lines[0].starts_with("Edits src/a<U+200B>.rs in the project folder /p:"),
            "{card}"
        );
        assert_eq!(
            lines[1..].to_vec(),
            vec![
                "Replace:",
                "let a = 1;<U+202E>",
                "With:",
                "let a = 2;<U+202E>",
                "Note: this request contains 3 hidden characters, shown as <U+…>.",
            ]
        );
    }

    #[test]
    fn a_string_over_the_budget_is_cut_with_the_cut_notice() {
        let long = "a".repeat(EDIT_PREVIEW_BYTES + 500);
        let card = edit_card("a.txt", None, Path::new("/p"), 1, 1, &long, "b");
        let lines: Vec<&str> = card.lines().collect();
        assert_eq!(lines[1], "Replace:");
        assert_eq!(lines[2], "a".repeat(EDIT_PREVIEW_BYTES));
        assert_eq!(lines[3], "... (500 more characters not shown)");
        assert_eq!(lines[4..].to_vec(), vec!["With:", "b"]);
    }
}
