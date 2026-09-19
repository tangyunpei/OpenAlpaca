//! Who can answer a tool-approval prompt (M6, ruling S10).
//!
//! A confirm-listed tool stops the run that called it and asks. The daemon
//! waits `confirmation_timeout_secs` (default 300 s) for an answer, **per tool
//! call** — so a client that cannot be asked has to say so, or a scripted run
//! spends five minutes per tool discovering it.
//!
//! The rule is the shape of this process's own streams: it can answer when
//! **stdin and stdout are both a terminal**, and not otherwise. Either one
//! redirected is enough to declare — a question written to a pipe is a hang,
//! and an answer read from one is worse. M6's first cut keyed on the verb
//! (`--message` was always unattended), which took the inline `[y/N]` away
//! from a one-shot typed at a prompt; S10 is that correction.
//!
//! It is a **declaration, not an approval**: the daemon refuses the call at
//! once and tells the model where it can be approved
//! (`openalpaca tasks confirmations …`). Nothing is pre-allowed here or there.

use std::io::IsTerminal;

/// Whether an invocation with these streams can answer a prompt.
///
/// Pure so the rule is testable without a pty; [`declared_unattended`] is the
/// one place that asks the real process.
pub fn can_answer_prompts(stdin_is_terminal: bool, stdout_is_terminal: bool) -> bool {
    stdin_is_terminal && stdout_is_terminal
}

/// What this process declares on a request body: `true` when nobody here can
/// be asked.
pub fn declared_unattended() -> bool {
    !can_answer_prompts(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S10: a terminal can answer, whatever verb it typed; a redirected
    /// stream cannot, whatever verb it typed.
    #[test]
    fn a_terminal_can_answer_and_a_redirected_stream_cannot() {
        assert!(can_answer_prompts(true, true));
        // stdout redirected — `> answer.txt`, `| jq`: the question would be
        // written into the file.
        assert!(!can_answer_prompts(true, false));
        // stdin redirected — `< question.txt`, a cron line: nobody is typing.
        assert!(!can_answer_prompts(false, true));
        // Neither: a CI step.
        assert!(!can_answer_prompts(false, false));
    }
}
