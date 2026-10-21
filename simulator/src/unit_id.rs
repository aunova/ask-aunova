//! What was heard, turned into the unit it names — or into nothing at all.
//!
//! A recogniser hands back a sentence, and a sentence is free text. Free text
//! inside the trust boundary is what this project exists to argue against, so
//! it is worth saying exactly why parsing one is acceptable *here* and would
//! not be on the other side of the wire. Three things are true of this module,
//! and the rest of the crate keeps them true:
//!
//! 1. **It is on the client side of the boundary.** It never decides what may
//!    be answered. It decides which question to offer to put.
//! 2. **Its output is shown and confirmed before anything is acted on.** The
//!    page prints what was heard and what it was taken to mean, and no route
//!    reaches `tools/call` until the person has confirmed it.
//! 3. **The server enforces unit scope independently.** A mis-normalisation
//!    cannot leak another unit's figures; it is refused like any other
//!    out-of-scope request, by the bearer token, server-side.
//!
//! So this module is allowed to be wrong. It is not allowed to be *confidently*
//! wrong, which is why it fails closed everywhere it is unsure:
//!
//! - a number that is not a unit resolves to nothing;
//! - two unit-like numbers in one utterance resolve to nothing, and are shown
//!   as the choice they are;
//! - a partial number — `twelve oh` — resolves to nothing;
//! - a unit this project does not have resolves to nothing.
//!
//! And the one rule that outranks the rest: **it never falls back to the
//! caller's own unit.** It cannot, structurally — nothing here takes an
//! [`Identity`](crate::identity::Identity), and there is no parameter to pass
//! one through. No match means no match.

use std::ops::Range;

use serde::Serialize;

/// Every unit of the demonstration project, as printed in its sale agreement.
///
/// A unit outside this list is refused rather than passed on: the page would
/// be offering to ask about something that does not exist. The list is held
/// against the dataset itself in `tests/voice.rs`, so it cannot quietly fall
/// behind it.
pub const UNITS: [&str; 3] = ["0802", "1204", "1510"];

/// The project, as it is said out loud.
pub const PROJECT_NAME: &str = "Palm Vista Tower";

/// Longest run of digits read as one number.
///
/// A unit id is four digits. This is above that so that a longer number is
/// *seen* and refused as "not a unit" rather than cut down to one.
const MAX_NUMBER_DIGITS: usize = 12;

/// What a transcript was taken to refer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// One unit of the project.
    Unit {
        /// The unit id, taken from [`UNITS`] and from nowhere else.
        id: &'static str,
    },
    /// The project itself, with no unit named.
    Project,
}

/// One reading the page may offer to put, once a person has confirmed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Choice {
    /// What it refers to.
    pub reference: Reference,
    /// What to show for it: `unit 1204`, or the project's name.
    pub label: String,
    /// The question to ask if this is the one — the transcript with the unit
    /// id written into it, so the loop reads the same unit the person saw.
    pub question: String,
}

/// How far a transcript got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Exactly one reference. It is still shown, and still confirmed.
    Resolved,
    /// More than one, so none was chosen.
    Ambiguous,
    /// Nothing resolved.
    Unresolved,
}

/// What was heard, and what it was taken to mean.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Interpretation {
    /// Every transcript that was considered, best first, exactly as heard.
    pub heard: Vec<String>,
    /// How far they got between them.
    pub outcome: Outcome,
    /// What the page may offer: one when resolved, two or more when
    /// ambiguous, none when nothing resolved.
    pub choices: Vec<Choice>,
    /// The question the page may put in the box for a single confirmation.
    ///
    /// `None` is the fail-closed case: the person picks a choice or types it.
    /// It is never the caller's own unit, and never a unit nobody said.
    pub question: Option<String>,
    /// What was understood, in one sentence, for the page to show.
    pub note: String,
}

/// Read every alternative a recogniser offered, and say what they amount to.
///
/// Alternatives that disagree are an ambiguity, not a ranking problem. The
/// distinct units they resolve to are collected, and only a set of exactly one
/// is a candidate; preferring the recogniser's first guess over its second
/// would be taking its opinion as evidence.
pub fn interpret(heard: &[String]) -> Interpretation {
    let readings: Vec<Reading<'_>> = heard.iter().map(|text| read(text)).collect();

    let mut units: Vec<Choice> = Vec::new();
    let mut project: Option<Choice> = None;
    let mut strays: Vec<String> = Vec::new();
    for reading in &readings {
        for found in &reading.units {
            if !units
                .iter()
                .any(|choice| choice.reference == Reference::Unit { id: found.id })
            {
                units.push(Choice {
                    reference: Reference::Unit { id: found.id },
                    label: format!("unit {}", found.id),
                    question: reading.rewritten(found),
                });
            }
        }
        if reading.project && project.is_none() {
            project = Some(Choice {
                reference: Reference::Project,
                label: PROJECT_NAME.to_owned(),
                question: reading.transcript.trim().to_owned(),
            });
        }
        for stray in &reading.strays {
            if !strays.contains(stray) {
                strays.push(stray.clone());
            }
        }
    }

    let heard: Vec<String> = heard.iter().map(|text| text.trim().to_owned()).collect();

    // A unit is the most specific thing that can be said, so a project named
    // alongside one is the same subject rather than a second one. Two *units*
    // are two subjects, and that is the ambiguity worth stopping for.
    if units.len() > 1 {
        let labels: Vec<&str> = units.iter().map(|choice| choice.label.as_str()).collect();
        let note = format!(
            "That could be {}. Pick the one you meant, or type it — this page will not choose \
             for you.",
            joined(&labels),
        );
        return Interpretation {
            heard,
            outcome: Outcome::Ambiguous,
            // Fail closed: no question is offered, because choosing one of
            // the two units the person may have said is a guess.
            question: None,
            note,
            choices: units,
        };
    }
    // `pop` rather than an index: there is no branch here that takes an
    // element out of a vector that might be empty, so there is none that can
    // panic in a request path.
    if let Some(choice) = units.pop().or(project) {
        return Interpretation {
            heard,
            outcome: Outcome::Resolved,
            question: Some(choice.question.clone()),
            note: match choice.reference {
                Reference::Unit { .. } => format!(
                    "Taken as {}. Nothing is asked until you confirm it.",
                    choice.label,
                ),
                Reference::Project => format!(
                    "Taken as a question about {}, with no unit named.",
                    choice.label,
                ),
            },
            choices: vec![choice],
        };
    }
    unresolved(heard, &strays)
}

/// Nothing resolved, said in the way that fits what was in the way.
fn unresolved(heard: Vec<String>, strays: &[String]) -> Interpretation {
    // Something with a letter or a digit in it is something that was said.
    // A transcript of punctuation is not a question to offer to ask.
    let said = heard
        .iter()
        .find(|text| {
            text.chars()
                .any(|character| character.is_ascii_alphanumeric())
        })
        .cloned();
    let Some(said) = said else {
        return Interpretation {
            heard,
            outcome: Outcome::Unresolved,
            choices: Vec::new(),
            question: None,
            note: "Nothing was heard. Say it again, or type the question.".to_owned(),
        };
    };
    if strays.is_empty() {
        // Nothing unit-like was said, so nothing is being refused: the
        // question stands as it was heard, and the loop will ask which unit
        // if it needs one. A unit is never invented to fill the gap.
        return Interpretation {
            heard,
            outcome: Outcome::Unresolved,
            choices: Vec::new(),
            question: Some(said),
            note: "No unit was named, so none was assumed. This will be asked exactly as it was \
                   heard."
                .to_owned(),
        };
    }
    let numbers: Vec<&str> = strays.iter().map(String::as_str).collect();
    Interpretation {
        heard,
        outcome: Outcome::Unresolved,
        choices: Vec::new(),
        // Fail closed: something number-shaped was said and it is not a unit
        // of this project. Offering the sentence anyway would be this page
        // deciding the number did not matter.
        question: None,
        note: format!(
            "I heard {}, which is not a unit of this project, so it was not taken as one. Type \
             the unit number if that is what you meant.",
            joined(&numbers),
        ),
    }
}

/// `a`, `a or b`, `a, b or c`.
fn joined(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// One unit id found in one transcript, and the run of text that named it.
#[derive(Debug)]
struct Found {
    /// The unit, from [`UNITS`].
    id: &'static str,
    /// The bytes that named it, in the transcript it was read from.
    span: Range<usize>,
    /// Whether the word immediately before it was already `unit`.
    after_the_word_unit: bool,
}

/// What one transcript carries.
#[derive(Debug)]
struct Reading<'a> {
    /// The transcript it was read from.
    transcript: &'a str,
    /// Every distinct unit named in it, in the order they were named.
    units: Vec<Found>,
    /// Whether the project was named.
    project: bool,
    /// Numbers that were said and are not units, as digits.
    strays: Vec<String>,
}

impl Reading<'_> {
    /// The transcript with a unit id written in where the number was said.
    ///
    /// `twelve oh four` becomes `unit 1204`, so the sentence the person
    /// confirms is the sentence the loop reads a unit out of. The word `unit`
    /// is added only when it was not already there, and never removed.
    fn rewritten(&self, found: &Found) -> String {
        let mut question = String::with_capacity(self.transcript.len() + "unit ".len());
        question.push_str(&self.transcript[..found.span.start]);
        if !found.after_the_word_unit {
            question.push_str("unit ");
        }
        question.push_str(found.id);
        question.push_str(&self.transcript[found.span.end..]);
        question.trim().to_owned()
    }
}

/// One alphanumeric run of a transcript.
#[derive(Debug)]
struct Token<'a> {
    text: &'a str,
    span: Range<usize>,
}

/// Read one transcript: every number in it, and whether it named the project.
fn read(transcript: &str) -> Reading<'_> {
    let tokens = tokens(transcript);
    let mut units: Vec<Found> = Vec::new();
    let mut strays = Vec::new();
    let mut index = 0usize;

    while index < tokens.len() {
        // A number far too long to be a unit is still a number that was
        // said: it is seen and refused rather than passed over as though it
        // had not been, and it is named by its length so that a very long
        // one cannot flood the sentence the page shows.
        let token = tokens[index].text;
        if token.len() > MAX_NUMBER_DIGITS && token.bytes().all(|byte| byte.is_ascii_digit()) {
            let stray = format!("a {}-digit number", token.len());
            if !strays.contains(&stray) {
                strays.push(stray);
            }
            index += 1;
            continue;
        }
        let Some(digits) = digits_of(tokens[index].text) else {
            index += 1;
            continue;
        };
        // A run is number words and digit groups with nothing but spacing
        // between them, so `twelve oh four` is one number and `1204 and 0802`
        // is two.
        let start = index;
        let mut number = digits;
        index += 1;
        while index < tokens.len()
            && spacing_only(&transcript[tokens[index - 1].span.end..tokens[index].span.start])
            && let Some(more) = digits_of(tokens[index].text)
        {
            if number.len() + more.len() > MAX_NUMBER_DIGITS {
                break;
            }
            number.push_str(&more);
            index += 1;
        }

        let span = tokens[start].span.start..tokens[index - 1].span.end;
        match UNITS.iter().find(|unit| **unit == number) {
            Some(id) => {
                let already = start > 0 && tokens[start - 1].text.eq_ignore_ascii_case("unit");
                if !units.iter().any(|found| found.id == *id) {
                    units.push(Found {
                        id,
                        span,
                        after_the_word_unit: already,
                    });
                }
            }
            None => {
                if !strays.contains(&number) {
                    strays.push(number);
                }
            }
        }
    }

    Reading {
        transcript,
        units,
        project: names_the_project(&tokens, transcript),
        strays,
    }
}

/// Every run of ASCII letters or digits, with where it sat.
///
/// Anything else — spacing, punctuation, or any non-ASCII character — is a
/// separator. Runs are cut on ASCII bytes, which never occur inside a
/// multi-byte UTF-8 sequence, so every span is on a character boundary.
fn tokens(transcript: &str) -> Vec<Token<'_>> {
    let bytes = transcript.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !bytes[cursor].is_ascii_alphanumeric() {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphanumeric() {
            cursor += 1;
        }
        tokens.push(Token {
            text: &transcript[start..cursor],
            span: start..cursor,
        });
    }
    tokens
}

/// The digits one token contributes to a number, if it contributes any.
///
/// Each word contributes its own digits in the order they are said, so
/// `twelve oh four` is `12`, `0`, `4`. A word outside the table contributes
/// nothing and ends the run, which is what makes `one and a half million`
/// fail to be a unit rather than become one.
fn digits_of(token: &str) -> Option<String> {
    if token.len() <= MAX_NUMBER_DIGITS && token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(token.to_owned());
    }
    NUMBER_WORDS
        .iter()
        .find(|(word, _)| token.eq_ignore_ascii_case(word))
        .map(|(_, digits)| (*digits).to_owned())
}

/// Whether two tokens are part of one spoken number rather than two.
fn spacing_only(between: &str) -> bool {
    !between.is_empty()
        && between
            .chars()
            .all(|character| character.is_whitespace() || character == '-' || character == ',')
}

/// The number words a recogniser writes out, and the digits each is.
///
/// `oh` is here because it is how a unit number is read aloud; `o` is not,
/// because a stray letter is not evidence that a digit was said.
const NUMBER_WORDS: [(&str, &str); 28] = [
    ("zero", "0"),
    ("nought", "0"),
    ("oh", "0"),
    ("one", "1"),
    ("two", "2"),
    ("three", "3"),
    ("four", "4"),
    ("five", "5"),
    ("six", "6"),
    ("seven", "7"),
    ("eight", "8"),
    ("nine", "9"),
    ("ten", "10"),
    ("eleven", "11"),
    ("twelve", "12"),
    ("thirteen", "13"),
    ("fourteen", "14"),
    ("fifteen", "15"),
    ("sixteen", "16"),
    ("seventeen", "17"),
    ("eighteen", "18"),
    ("nineteen", "19"),
    ("twenty", "20"),
    ("thirty", "30"),
    ("forty", "40"),
    ("fifty", "50"),
    ("sixty", "60"),
    ("eighty", "80"),
];

/// Whether a transcript named the project.
///
/// `Palm Vista`, `palm-vista`, `Palm Vista Tower` and the one word a
/// recogniser sometimes runs together. Nothing looser: a project reference
/// that matched `palm` alone would turn a question about palm trees into a
/// question about this building.
fn names_the_project(tokens: &[Token<'_>], transcript: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        if token.text.eq_ignore_ascii_case("palmvista") {
            return true;
        }
        token.text.eq_ignore_ascii_case("palm")
            && tokens.get(index + 1).is_some_and(|next| {
                next.text.eq_ignore_ascii_case("vista")
                    && spacing_only(&transcript[token.span.end..next.span.start])
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a row of the table expects.
    #[derive(Debug)]
    enum Expect {
        /// Exactly this unit, and a question the loop can read it out of.
        Unit(&'static str),
        /// The project, with no unit named.
        Project,
        /// More than one unit: shown as a choice, never chosen.
        Ambiguous(&'static [&'static str]),
        /// Nothing resolved, and nothing offered to send either.
        RefusedAndNotOffered,
        /// Nothing unit-like was said: the sentence stands, with no unit in it.
        NoUnitNamed,
    }

    /// Every case this normaliser is held to, matches and refusals together.
    ///
    /// The refusals are the half that matters: a normaliser tested only on
    /// what it should accept has not been tested.
    const TABLE: &[(&str, Expect)] = &[
        // --- the confident cases ---------------------------------------
        ("twelve oh four", Expect::Unit("1204")),
        ("1204", Expect::Unit("1204")),
        ("12 04", Expect::Unit("1204")),
        ("one two zero four", Expect::Unit("1204")),
        ("oh eight oh two", Expect::Unit("0802")),
        ("fifteen ten", Expect::Unit("1510")),
        ("zero eight zero two", Expect::Unit("0802")),
        (
            "what is the payment status of unit twelve oh four",
            Expect::Unit("1204"),
        ),
        (
            "What's the payment status of unit 0802?",
            Expect::Unit("0802"),
        ),
        (
            "payment status for twelve-oh-four please",
            Expect::Unit("1204"),
        ),
        (
            "my statement says unit 1204 has paid 1,470,000.00 dirhams",
            Expect::Unit("1204"),
        ),
        ("Palm Vista", Expect::Project),
        ("is palm vista on track", Expect::Project),
        (
            "is the Palm Vista Tower structure certified",
            Expect::Project,
        ),
        ("how is palm-vista doing", Expect::Project),
        ("anything new on palmvista", Expect::Project),
        // A unit named alongside the project is still that unit: the project
        // is the same subject, not a second one.
        ("palm vista unit fifteen ten", Expect::Unit("1510")),
        // --- the refusals ----------------------------------------------
        // A partial number is not a unit, and must not be completed.
        ("twelve oh", Expect::RefusedAndNotOffered),
        ("unit twelve", Expect::RefusedAndNotOffered),
        // A unit this project does not have.
        ("0901", Expect::RefusedAndNotOffered),
        ("unit oh nine oh one", Expect::RefusedAndNotOffered),
        // A number that is not a unit at all.
        ("thirty four storeys", Expect::RefusedAndNotOffered),
        (
            "I have paid 1,470,000.00 dirhams",
            Expect::RefusedAndNotOffered,
        ),
        ("about one and a half million", Expect::RefusedAndNotOffered),
        // Two unit-like numbers in one utterance: a choice, never a guess.
        (
            "unit 1204 and unit 0802",
            Expect::Ambiguous(&["1204", "0802"]),
        ),
        (
            "twelve oh four or fifteen ten",
            Expect::Ambiguous(&["1204", "1510"]),
        ),
        // Noise.
        ("", Expect::RefusedAndNotOffered),
        ("   ", Expect::RefusedAndNotOffered),
        ("uh", Expect::NoUnitNamed),
        ("...", Expect::RefusedAndNotOffered),
        // Nothing unit-like: the question stands, and no unit is invented.
        ("is the project on track", Expect::NoUnitNamed),
        ("am I behind on payments", Expect::NoUnitNamed),
        ("what has changed since last time", Expect::NoUnitNamed),
    ];

    /// Every unit id the table is allowed to produce for a row, which is only
    /// the ones the row's own words could have named.
    fn units_in(interpretation: &Interpretation) -> Vec<String> {
        interpretation
            .choices
            .iter()
            .filter_map(|choice| match choice.reference {
                Reference::Unit { id } => Some(id.to_owned()),
                Reference::Project => None,
            })
            .collect()
    }

    #[test]
    fn the_table() {
        for (transcript, expect) in TABLE {
            let read = interpret(&[(*transcript).to_owned()]);
            match expect {
                Expect::Unit(id) => {
                    assert_eq!(read.outcome, Outcome::Resolved, "`{transcript}`: {read:?}");
                    assert_eq!(units_in(&read), vec![(*id).to_owned()], "`{transcript}`");
                    let question = read.question.as_deref().unwrap_or_default();
                    assert!(
                        question.contains(&format!("unit {id}")),
                        "`{transcript}` must be rewritten into something the loop can read a \
                         unit out of: `{question}`",
                    );
                }
                Expect::Project => {
                    assert_eq!(read.outcome, Outcome::Resolved, "`{transcript}`: {read:?}");
                    assert_eq!(
                        read.choices.iter().map(|c| c.reference).collect::<Vec<_>>(),
                        vec![Reference::Project],
                        "`{transcript}`",
                    );
                    assert_eq!(read.question.as_deref(), Some(transcript.trim()));
                }
                Expect::Ambiguous(ids) => {
                    assert_eq!(read.outcome, Outcome::Ambiguous, "`{transcript}`: {read:?}");
                    assert_eq!(units_in(&read), *ids, "`{transcript}`");
                    assert_eq!(
                        read.question, None,
                        "`{transcript}` must offer no single question to send: {read:?}",
                    );
                    for id in *ids {
                        assert!(
                            read.note.contains(id),
                            "`{transcript}` must show both units it could be: {}",
                            read.note,
                        );
                    }
                }
                Expect::RefusedAndNotOffered => {
                    assert_eq!(
                        read.outcome,
                        Outcome::Unresolved,
                        "`{transcript}`: {read:?}"
                    );
                    assert!(read.choices.is_empty(), "`{transcript}`: {read:?}");
                    assert_eq!(
                        read.question, None,
                        "`{transcript}` resolved to nothing, so nothing may be offered to send",
                    );
                }
                Expect::NoUnitNamed => {
                    assert_eq!(
                        read.outcome,
                        Outcome::Unresolved,
                        "`{transcript}`: {read:?}"
                    );
                    assert!(read.choices.is_empty(), "`{transcript}`: {read:?}");
                    assert_eq!(
                        read.question.as_deref(),
                        Some(transcript.trim()),
                        "a sentence with no unit in it is asked as it was heard",
                    );
                }
            }
        }
    }

    #[test]
    fn the_refusals_are_half_the_table() {
        // Not a round number for its own sake: the point is that the refusals
        // are a first-class half of this table and stay that way.
        let refusals = TABLE
            .iter()
            .filter(|(_, expect)| {
                matches!(
                    expect,
                    Expect::RefusedAndNotOffered | Expect::Ambiguous(_) | Expect::NoUnitNamed
                )
            })
            .count();
        assert_eq!(TABLE.len(), 33, "the table size is part of what is claimed");
        assert_eq!(refusals, 16, "the rows that must not resolve to a unit");
    }

    #[test]
    fn no_row_ever_produces_a_unit_its_own_words_did_not_name() {
        // The R3 defect, as an invariant over the whole table: a unit id may
        // appear in an interpretation only if the transcript could have named
        // it. Nothing here knows the caller's unit, and this is what proves
        // it stays that way.
        for (transcript, _) in TABLE {
            let read = interpret(&[(*transcript).to_owned()]);
            let rendered = serde_json::to_string(&read).expect("an interpretation serialises");
            for unit in UNITS {
                if rendered.contains(unit) {
                    let said = read.choices.iter().any(
                        |choice| matches!(choice.reference, Reference::Unit { id } if id == unit),
                    );
                    assert!(
                        said,
                        "`{transcript}` put {unit} in its output without resolving it: \
                         {rendered}",
                    );
                }
            }
        }
    }

    #[test]
    fn a_question_about_my_own_unit_never_becomes_a_question_about_1204() {
        // The buyer's demo token is bound to 1204, and its name even carries
        // the number. This module has no way to reach it, and this is the
        // test that fails if anybody ever gives it one.
        for mine in [
            "what is my payment status",
            "am I behind on my payments",
            "what do I still owe on my apartment",
        ] {
            let read = interpret(&[mine.to_owned()]);
            assert!(read.choices.is_empty(), "`{mine}`: {read:?}");
            assert_eq!(read.question.as_deref(), Some(mine));
            for unit in UNITS {
                assert!(
                    !read.note.contains(unit),
                    "`{mine}` must not name {unit}: {}",
                    read.note,
                );
            }
        }
    }

    #[test]
    fn alternatives_that_agree_resolve_and_alternatives_that_disagree_do_not() {
        // Five alternatives, one unit between them: the recogniser spelt it
        // three ways and meant the same thing once.
        let agreeing = interpret(&[
            "payment status for twelve oh for".to_owned(),
            "payment status for twelve oh four".to_owned(),
            "payment status for 12 04".to_owned(),
        ]);
        assert_eq!(agreeing.outcome, Outcome::Resolved);
        assert_eq!(units_in(&agreeing), vec!["1204".to_owned()]);
        assert_eq!(agreeing.heard.len(), 3, "every alternative is shown");

        // Two alternatives, two different units. The first is the
        // recogniser's best guess, and that is exactly why it is not taken.
        let disagreeing = interpret(&[
            "payment status for unit twelve oh four".to_owned(),
            "payment status for unit oh eight oh two".to_owned(),
        ]);
        assert_eq!(disagreeing.outcome, Outcome::Ambiguous);
        assert_eq!(
            units_in(&disagreeing),
            vec!["1204".to_owned(), "0802".to_owned()],
        );
        assert_eq!(
            disagreeing.question, None,
            "the best-ranked alternative is an opinion, not evidence: {disagreeing:?}",
        );
    }

    #[test]
    fn no_alternatives_at_all_is_nothing_heard() {
        let nothing = interpret(&[]);
        assert_eq!(nothing.outcome, Outcome::Unresolved);
        assert_eq!(nothing.question, None);
        assert!(
            nothing.note.contains("Nothing was heard"),
            "{}",
            nothing.note
        );
    }

    #[test]
    fn a_rewritten_question_keeps_everything_that_was_said_around_the_number() {
        let read = interpret(&["so, is unit twelve oh four paid up to date?".to_owned()]);
        assert_eq!(
            read.question.as_deref(),
            Some("so, is unit 1204 paid up to date?"),
            "{read:?}",
        );
    }

    #[test]
    fn a_unit_spoken_without_the_word_unit_gains_it_and_nothing_else() {
        let read = interpret(&["is twelve oh four paid up".to_owned()]);
        assert_eq!(read.question.as_deref(), Some("is unit 1204 paid up"));
    }

    #[test]
    fn a_transcript_that_is_not_ascii_is_cut_on_character_boundaries() {
        // Slicing this on byte offsets taken from a lowercased copy is how a
        // normaliser panics in front of an audience.
        let read = interpret(&["Grüße — payment status for twelve oh four, danke".to_owned()]);
        assert_eq!(
            read.question.as_deref(),
            Some("Grüße — payment status for unit 1204, danke"),
        );
    }

    #[test]
    fn an_over_long_run_of_digits_is_not_cut_down_into_a_unit() {
        // Neither of these may become 1204 by truncation at any point, and
        // neither may be passed over as though no number had been said: both
        // must come back as a refusal with nothing offered to send.
        for long in ["120412041204", "12041204120412041204"] {
            let read = interpret(&[long.to_owned()]);
            assert_eq!(read.outcome, Outcome::Unresolved, "{long}: {read:?}");
            assert!(read.choices.is_empty(), "{long}: {read:?}");
            assert_eq!(read.question, None, "{long}: {read:?}");
        }
    }

    #[test]
    fn every_unit_in_the_table_is_four_digits_as_the_dataset_prints_them() {
        for unit in UNITS {
            assert_eq!(unit.len(), 4, "{unit}");
            assert!(unit.bytes().all(|byte| byte.is_ascii_digit()), "{unit}");
        }
    }
}
