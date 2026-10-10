//! What an approval card says about where a call acts (storage design §12,
//! R34): the daemon's own text, shown above the call's arguments
//! (`BuiltInTool::confirmation_preview`). A folder prints as it is: a
//! project's root is the owner's own path, and a scratch folder or a store is
//! OpenAlpaca's. A path the model wrote is third-party text and goes through
//! `utils::text`, with the notice under it (CLAUDE.md, Gotchas).

use crate::tools::working_folder::WorkingFolder;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
