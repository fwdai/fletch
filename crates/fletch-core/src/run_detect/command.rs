//! The program a Run command starts.

/// The program `cmd` starts: its first word past any leading `NAME=value`
/// assignments, as the shell would resolve it — `pnpm` of `pnpm dev`,
/// `node` of `PORT=3000 node server.js`, `cd` of `cd app && npm run dev`,
/// whose failure the shell reports itself. `None` for a command with no such
/// word, and for a first word the shell takes apart before it looks the
/// program up (a quote, an expansion, an operator): what that names is the
/// shell's to find out.
pub(crate) fn program(cmd: &str) -> Option<&str> {
    const TAKEN_APART: &[char] = &[
        '"', '\'', '\\', '$', '`', '(', ')', '{', '}', '|', '&', ';', '<', '>', '!',
    ];
    let word = cmd.split_whitespace().find(|word| !is_assignment(word))?;
    (!word.contains(TAKEN_APART)).then_some(word)
}

/// Whether `word` is a `NAME=value` the shell puts in the command's
/// environment rather than runs.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_the_first_word_past_the_assignments() {
        assert_eq!(program("pnpm dev"), Some("pnpm"));
        assert_eq!(program("npm run dev -- --port 3000"), Some("npm"));
        assert_eq!(
            program("PORT=3000 NODE_ENV=dev node server.js"),
            Some("node")
        );
        assert_eq!(program("  cargo run  "), Some("cargo"));
        assert_eq!(program("./scripts/dev.sh"), Some("./scripts/dev.sh"));
        assert_eq!(
            program("~/.cargo/bin/cargo run"),
            Some("~/.cargo/bin/cargo")
        );
        // A list: the first command's program is still the first to run.
        assert_eq!(program("cd app && npm run dev"), Some("cd"));
        // Shell syntax later in the line changes nothing about the program.
        assert_eq!(program("node -e \"require('./s')\" > log"), Some("node"));
    }

    #[test]
    fn a_first_word_the_shell_takes_apart_names_no_program() {
        assert_eq!(program(""), None);
        assert_eq!(program("   "), None);
        assert_eq!(program("PORT=3000"), None);
        assert_eq!(program("\"my program\" dev"), None);
        assert_eq!(program("$PM dev"), None);
        assert_eq!(program("(cd app && npm run dev)"), None);
        assert_eq!(program("! npm test"), None);
        // `1=x` is no assignment: a word the shell would run.
        assert_eq!(program("1=x dev"), Some("1=x"));
    }
}
