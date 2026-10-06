//! Where "the user said it" is decided — and the only place it can be.
//!
//! Trust in the context layer lives in an assertion's `source`: a `user_turn`
//! source lands confirmed, as the user's own word. That makes the claim
//! worth forging, so no writer
//! may assert it: a model's `stated_by_user: true` is just more model output.
//! Instead a writer hands over the user's words and this module looks for
//! them, verbatim, in the user's own turns. Only a match yields a
//! [`UserStated`], and only a [`UserStated`] yields a `user_turn` source — a
//! test greps the crate for any other construction of that source kind.

use super::model::{Source, SourceKind};

/// One of the user's turns, as text, for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTurnText {
    pub turn_id: String,
    pub text: String,
}

/// Proof that a quote was found in a user turn. Constructible only here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserStated {
    turn_id: String,
}

impl UserStated {
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// The source a user-stated record carries.
    pub fn source(&self) -> Source {
        Source {
            kind: SourceKind::UserTurn,
            reference: Some(self.turn_id.clone()),
        }
    }
}

/// Shortest quote that counts: anything less matches by accident.
pub const MIN_QUOTE_CHARS: usize = 12;

/// Find `quote` in the user's turns. The match is on normalised text
/// (lowercase, whitespace collapsed, surrounding quotes and punctuation
/// dropped), so a model's light re-typing still matches and a paraphrase
/// does not. Newest turn first, so the reference names where the user said
/// it last.
pub fn find_user_quote(turns: &[UserTurnText], quote: &str) -> Option<UserStated> {
    let needle = normalise(quote);
    if needle.chars().count() < MIN_QUOTE_CHARS {
        return None;
    }
    turns
        .iter()
        .rev()
        .find(|t| normalise(&t.text).contains(&needle))
        .map(|t| UserStated {
            turn_id: t.turn_id.clone(),
        })
}

/// Lowercase, single-spaced, without the quote marks and trailing
/// punctuation a model wraps a citation in.
pub fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | '`'))
        .trim_end_matches(['.', '!', '?', ',', ';', ':'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turns() -> Vec<UserTurnText> {
        vec![
            UserTurnText {
                turn_id: "t1".into(),
                text: "Let's use JWT for sessions.".into(),
            },
            UserTurnText {
                turn_id: "t2".into(),
                text: "And never store tokens in local storage — that's a rule.".into(),
            },
        ]
    }

    #[test]
    fn a_verbatim_quote_names_the_turn_it_came_from() {
        let found = find_user_quote(&turns(), "never store tokens in local storage").unwrap();
        assert_eq!(found.turn_id(), "t2");
        assert_eq!(found.source().kind, SourceKind::UserTurn);
        assert_eq!(found.source().reference.as_deref(), Some("t2"));
    }

    #[test]
    fn light_retyping_matches_and_paraphrase_does_not() {
        assert!(find_user_quote(&turns(), "  “NEVER store   tokens in local storage.” ").is_some());
        assert!(find_user_quote(&turns(), "tokens must stay out of local storage").is_none());
    }

    #[test]
    fn a_short_quote_never_counts() {
        assert!(find_user_quote(&turns(), "use JWT").is_none());
        assert!(find_user_quote(&turns(), "").is_none());
    }

    #[test]
    fn the_newest_turn_wins_a_tie() {
        let mut t = turns();
        t.push(UserTurnText {
            turn_id: "t3".into(),
            text: "As I said: never store tokens in local storage.".into(),
        });
        assert_eq!(
            find_user_quote(&t, "never store tokens in local storage")
                .unwrap()
                .turn_id(),
            "t3"
        );
    }
}
