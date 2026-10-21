//! Turning what a person said about money into the integer the server takes.
//!
//! `verify_claim` compares an *integer count of minor units*: one dirham is
//! 100 fils, so `AED 1,470,000.00` is `147000000`. Everything else the server
//! can recognise but will not guess at comes back `UNVERIFIED`
//! `assertion_not_normalised` — which is a refusal to guess, not a verdict
//! about the project. On camera that looks like a broken demo rather than a
//! wrong answer, which is exactly why the conversion lives here, in one tested
//! function, instead of being left to a prompt.
//!
//! The arithmetic is integer throughout. A currency amount never touches
//! `f64`: `1.47 million` is parsed as the digits `147` shifted six places, not
//! as a float multiplied by a million.
//!
//! What it accepts, and nothing else:
//!
//! - digits, with optional `,` or space thousands separators: `1,470,000`;
//! - at most one decimal point, with no more digits after it than the currency
//!   has minor units: `1,470,000.00`, but not `1.475` dirhams;
//! - one optional scale word: `thousand`, `million`, `billion`;
//! - the currency written before or after, as `AED`, `dirham`, `dirhams`,
//!   or the symbol-free spoken forms a browser's speech recogniser produces.
//!
//! What it refuses, on purpose: a hedge. "about one and a half million" has no
//! exact value to assert, and a near miss comes back `CONTRADICTED`, which
//! would be a true statement about the wrong question. The caller is told to
//! ask which figure was meant rather than being handed a number nobody said.

/// Minor units in one major unit of the currency the dataset is denominated in.
///
/// One dirham is 100 fils. The server states this in every payment answer as
/// `currency_minor_units_per_major`; it is repeated here because the
/// conversion has to happen before the first call, when no answer has been
/// read yet.
pub const MINOR_UNITS_PER_MAJOR: i128 = 100;

/// Why an amount could not be converted.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MoneyError {
    /// No digits at all.
    #[error("`{spoken}` states no figure to check")]
    NoFigure { spoken: String },
    /// More than one figure, so which one is being asserted is a guess.
    #[error("`{spoken}` states more than one figure; ask which one is being claimed")]
    Ambiguous { spoken: String },
    /// A hedge: "about", "roughly", "a million and a half".
    #[error("`{spoken}` is approximate; there is no exact value to put to the server")]
    Approximate { spoken: String },
    /// More precision than the currency has.
    #[error(
        "`{spoken}` is finer than one minor unit of the currency, which has 100 per major unit"
    )]
    TooPrecise { spoken: String },
    /// Larger than the integer the server is given.
    #[error("`{spoken}` is larger than this client can state as minor units")]
    TooLarge { spoken: String },
}

/// Words that mean the figure is not exact.
///
/// Matched as whole words, never as substrings: "sponsor" must not read as a
/// hedge because "or" is inside it.
const HEDGES: [&str; 7] = [
    "about",
    "around",
    "roughly",
    "approximately",
    "circa",
    "nearly",
    "almost",
];

/// Scale words and the power of ten each multiplies by.
const SCALES: [(&str, u32); 3] = [("thousand", 3), ("million", 6), ("billion", 9)];

/// Convert one spoken or typed amount into an integer count of minor units.
///
/// The returned string is what goes into `asserted_value`: digits only, no
/// separators, no decimal point, no currency.
pub fn to_minor_units(spoken: &str) -> Result<String, MoneyError> {
    let lowered = spoken.to_lowercase();
    if lowered
        .split(|character: char| !character.is_ascii_alphabetic())
        .any(|word| HEDGES.contains(&word))
    {
        return Err(MoneyError::Approximate {
            spoken: spoken.to_owned(),
        });
    }

    let (figure, after_figure) = single_figure(&lowered, spoken)?;
    let scale = scale_of(&lowered, after_figure);

    let (whole, fraction) = match figure.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (figure.as_str(), ""),
    };
    // `1,470,000.00` is the digits `147000000` with the decimal point
    // forgotten, and the point's position recorded as a shift. Nothing here
    // divides, so nothing here rounds.
    let digits: String = format!("{whole}{fraction}");
    let shift = i64::from(scale) + MINOR_UNITS_PER_MAJOR.ilog10() as i64 - fraction.len() as i64;
    if shift < 0 {
        return Err(MoneyError::TooPrecise {
            spoken: spoken.to_owned(),
        });
    }

    let value: i128 = digits.parse().map_err(|_| MoneyError::TooLarge {
        spoken: spoken.to_owned(),
    })?;
    let multiplier = 10_i128
        .checked_pow(u32::try_from(shift).map_err(|_| MoneyError::TooLarge {
            spoken: spoken.to_owned(),
        })?)
        .ok_or_else(|| MoneyError::TooLarge {
            spoken: spoken.to_owned(),
        })?;
    let minor = value
        .checked_mul(multiplier)
        .ok_or_else(|| MoneyError::TooLarge {
            spoken: spoken.to_owned(),
        })?;
    Ok(minor.to_string())
}

/// The one figure in the text, with separators removed, and where it ended.
///
/// Two figures is not a parse to pick between: a claim with two numbers in it
/// is two claims, and the caller is told to say which.
///
/// The end offset is into `lowered`, and is what [`scale_of`] needs: a scale
/// word only scales the figure it is attached to.
fn single_figure(lowered: &str, spoken: &str) -> Result<(String, usize), MoneyError> {
    let mut figures: Vec<(String, usize)> = Vec::new();
    let mut current = String::new();
    let mut end = 0usize;
    let mut previous_was_digit = false;

    for (at, character) in lowered.char_indices() {
        if character.is_ascii_digit() {
            current.push(character);
            end = at + character.len_utf8();
            previous_was_digit = true;
            continue;
        }
        // A separator inside a run of digits belongs to the figure: `1,470`
        // and `1 470` are one number, `1,470 and 2` are two.
        if (character == ',' || character == ' ') && previous_was_digit {
            previous_was_digit = false;
            continue;
        }
        if character == '.' && previous_was_digit && !current.contains('.') {
            current.push('.');
            end = at + character.len_utf8();
            previous_was_digit = false;
            continue;
        }
        if !current.is_empty() {
            figures.push((std::mem::take(&mut current), end));
        }
        previous_was_digit = false;
    }
    if !current.is_empty() {
        figures.push((current, end));
    }

    let mut figures = figures.into_iter().map(|(figure, end)| {
        // A trailing point was sentence punctuation, not a decimal point.
        (figure.trim_end_matches('.').to_owned(), end)
    });
    let first = figures.next().ok_or_else(|| MoneyError::NoFigure {
        spoken: spoken.to_owned(),
    })?;
    if figures.next().is_some() {
        return Err(MoneyError::Ambiguous {
            spoken: spoken.to_owned(),
        });
    }
    Ok(first)
}

/// The power of ten named by a scale word **attached to the figure**, or zero.
///
/// Only the word immediately after the figure can multiply it, with nothing
/// but spacing or a hyphen between them. A scale word anywhere else in the
/// sentence is a word about something else: `multi-million` is not a figure's
/// scale, and neither is "a million" in the next clause. Reaching across a
/// sentence for one would multiply a number nobody scaled by a thousand or
/// more, and send it to the server as a claim the person never made.
fn scale_of(lowered: &str, after_figure: usize) -> u32 {
    let rest = lowered.get(after_figure..).unwrap_or_default();
    let word = rest
        .trim_start_matches([' ', '-'])
        .split(|character: char| !character.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    // `1.47 millions` is the same claim as `1.47 million`; `dirhams` is not.
    let word = word.strip_suffix('s').unwrap_or(word);
    SCALES
        .iter()
        .find(|(scale, _)| *scale == word)
        .map_or(0, |(_, power)| *power)
}

/// How a figure in minor units is said out loud.
///
/// The record holds `147000000`, and nobody hears that as what it is: one
/// million four hundred seventy thousand dirhams. The integer stays the
/// integer everywhere it matters — it is what `verify_claim` compares, and it
/// is what the drawer shows — and this is only the form it is read to a
/// person in.
///
/// Integer arithmetic throughout, like every other amount in this crate: a
/// currency value never touches `f64`, not even to be made readable.
///
/// `None` for a figure this cannot say — negative, or larger than the scale
/// words reach. A figure that cannot be said is left exactly as the server
/// wrote it rather than said wrongly.
pub fn in_words(minor: i128) -> Option<String> {
    if minor < 0 {
        return None;
    }
    let major = minor / MINOR_UNITS_PER_MAJOR;
    let fils = minor % MINOR_UNITS_PER_MAJOR;

    let mut said = whole_in_words(major)?;
    said.push_str(if major == 1 { " dirham" } else { " dirhams" });
    if fils > 0 {
        said.push_str(" and ");
        said.push_str(&whole_in_words(fils)?);
        said.push_str(if fils == 1 { " fil" } else { " fils" });
    }
    Some(said)
}

/// The scale word for each group of three digits, smallest first.
const SCALE_NAMES: [&str; 5] = ["", "thousand", "million", "billion", "trillion"];

/// Nought to nineteen, which is every number English does not build.
const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

/// The tens, indexed by the tens digit.
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// A whole number in words, or `None` if it is longer than English is given
/// a scale word for here.
fn whole_in_words(value: i128) -> Option<String> {
    if value == 0 {
        return Some("zero".to_owned());
    }
    let mut groups: Vec<u16> = Vec::new();
    let mut rest = value;
    while rest > 0 {
        // Each group is 0..=999, which is the only range `group_in_words`
        // reads, so every index it takes is inside its tables.
        groups.push((rest % 1_000) as u16);
        rest /= 1_000;
    }
    if groups.len() > SCALE_NAMES.len() {
        return None;
    }

    let mut said = String::new();
    for (index, group) in groups.iter().enumerate().rev() {
        if *group == 0 {
            continue;
        }
        if !said.is_empty() {
            said.push(' ');
        }
        said.push_str(&group_in_words(*group));
        if index > 0 {
            said.push(' ');
            said.push_str(SCALE_NAMES[index]);
        }
    }
    Some(said)
}

/// One group of three digits, `1..=999`, in words.
fn group_in_words(group: u16) -> String {
    let mut said = String::new();
    let hundreds = usize::from(group / 100);
    let rest = usize::from(group % 100);
    if hundreds > 0 {
        said.push_str(ONES[hundreds]);
        said.push_str(" hundred");
    }
    if rest > 0 {
        if !said.is_empty() {
            said.push(' ');
        }
        if rest < ONES.len() {
            said.push_str(ONES[rest]);
        } else {
            said.push_str(TENS[rest / 10]);
            if rest % 10 > 0 {
                said.push('-');
                said.push_str(ONES[rest % 10]);
            }
        }
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_written_form_on_a_statement() {
        assert_eq!(
            to_minor_units("AED 1,470,000.00").as_deref(),
            Ok("147000000"),
        );
    }

    #[test]
    fn converts_the_spoken_form_a_recogniser_produces() {
        assert_eq!(
            to_minor_units("1.47 million dirhams").as_deref(),
            Ok("147000000"),
        );
        assert_eq!(
            to_minor_units("1,470,000 dirhams").as_deref(),
            Ok("147000000"),
        );
        assert_eq!(
            to_minor_units("2.45 million AED").as_deref(),
            Ok("245000000")
        );
    }

    #[test]
    fn a_bare_integer_is_major_units_not_minor() {
        // The trap the skill file warns about: `1470000` sent straight through
        // asserts 14,700.00, and comes back CONTRADICTED.
        assert_eq!(to_minor_units("1470000").as_deref(), Ok("147000000"));
    }

    #[test]
    fn keeps_the_fils() {
        assert_eq!(to_minor_units("1,470,000.07").as_deref(), Ok("147000007"));
        assert_eq!(to_minor_units("0.01 dirhams").as_deref(), Ok("1"));
    }

    #[test]
    fn refuses_a_hedge_rather_than_inventing_a_figure() {
        assert_eq!(
            to_minor_units("about one and a half million"),
            Err(MoneyError::Approximate {
                spoken: "about one and a half million".to_owned()
            }),
        );
    }

    #[test]
    fn refuses_two_figures() {
        assert_eq!(
            to_minor_units("1,470,000 of 2,450,000"),
            Err(MoneyError::Ambiguous {
                spoken: "1,470,000 of 2,450,000".to_owned()
            }),
        );
    }

    #[test]
    fn refuses_more_precision_than_the_currency_has() {
        assert_eq!(
            to_minor_units("1.475 dirhams"),
            Err(MoneyError::TooPrecise {
                spoken: "1.475 dirhams".to_owned()
            }),
        );
    }

    #[test]
    fn refuses_text_with_no_figure_in_it() {
        assert_eq!(
            to_minor_units("what have I paid"),
            Err(MoneyError::NoFigure {
                spoken: "what have I paid".to_owned()
            }),
        );
    }

    #[test]
    fn a_sentence_full_stop_is_not_a_decimal_point() {
        assert_eq!(
            to_minor_units("I have paid 1,470,000 dirhams.").as_deref(),
            Ok("147000000"),
        );
    }

    #[test]
    fn stays_exact_far_above_what_a_float_can_hold() {
        // 2^53 minor units is where an f64 is already below one ulp. This is
        // larger, and it is exact.
        assert_eq!(
            to_minor_units("99,007,199,254,740,993.07").as_deref(),
            Ok("9900719925474099307"),
        );
    }

    /// What a row of the scale table expects.
    #[derive(Debug)]
    enum Expect {
        /// This exact count of minor units, and nothing else.
        Minor(&'static str),
        /// No figure to assert at all.
        Refused,
    }

    /// Every scale-word case, the ones that scale and the ones that must not.
    ///
    /// The negative rows are the half that matters. A scale word that reaches
    /// across a sentence multiplies a figure nobody scaled: `1,470,000` in a
    /// sentence that happens to contain `multi-million` would go to the
    /// server as 1.47 × 10^14 minor units, which is a claim the person never
    /// made and which comes back CONTRADICTED.
    const SCALE_TABLE: &[(&str, Expect)] = &[
        // --- attached, so it scales ------------------------------------
        ("1.47 million dirhams", Expect::Minor("147000000")),
        ("1.47-million", Expect::Minor("147000000")),
        ("1.47million", Expect::Minor("147000000")),
        ("1.47 millions", Expect::Minor("147000000")),
        ("2 thousand dirhams", Expect::Minor("200000")),
        ("3 billion", Expect::Minor("300000000000")),
        // --- not attached, so it must not scale ------------------------
        // A scale word inside another word.
        (
            "1,470,000 dirhams towards a multi-million tower",
            Expect::Minor("147000000"),
        ),
        ("1.47 multi-million", Expect::Minor("147")),
        // A scale word before the figure.
        (
            "a million-dirham unit, 1,470,000 paid",
            Expect::Minor("147000000"),
        ),
        ("million 1.47", Expect::Minor("147")),
        // A scale word in another clause.
        (
            "1.47, which is nothing like a million",
            Expect::Minor("147"),
        ),
        (
            "1.47 dirhams of a million-dirham unit",
            Expect::Minor("147"),
        ),
        // Two figures and one scale word: which one it belongs to is not
        // this client's to pick.
        ("1.47 million and 2", Expect::Refused),
        ("2 and 1.47 million", Expect::Refused),
        // A scale word and no figure at all.
        ("a million", Expect::Refused),
        ("multi-million", Expect::Refused),
    ];

    #[test]
    fn the_scale_table() {
        for (spoken, expect) in SCALE_TABLE {
            let converted = to_minor_units(spoken);
            match expect {
                Expect::Minor(minor) => {
                    assert_eq!(converted.as_deref(), Ok(*minor), "`{spoken}`");
                }
                Expect::Refused => {
                    assert!(
                        converted.is_err(),
                        "`{spoken}` has no one figure to assert: {converted:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn the_scale_table_refuses_as_many_rows_as_it_accepts() {
        let unscaled = SCALE_TABLE
            .iter()
            .filter(|(_, expect)| matches!(expect, Expect::Minor("147") | Expect::Refused))
            .count();
        assert_eq!(
            SCALE_TABLE.len(),
            16,
            "the table size is part of what is claimed",
        );
        assert_eq!(
            unscaled, 8,
            "the rows where a scale word must not reach the figure",
        );
    }

    #[test]
    fn a_figure_is_said_as_dirhams_and_never_as_a_count_of_fils() {
        // The figure from the dataset, and the sentence the server states it
        // in: `147000000` is not a number anybody hears as money.
        assert_eq!(
            in_words(147_000_000).as_deref(),
            Some("one million four hundred seventy thousand dirhams"),
        );
        assert_eq!(
            in_words(198_000_000).as_deref(),
            Some("one million nine hundred eighty thousand dirhams"),
        );
        assert_eq!(
            in_words(89_100_000).as_deref(),
            Some("eight hundred ninety-one thousand dirhams"),
        );
    }

    #[test]
    fn the_fils_are_said_when_there_are_any_and_left_out_when_there_are_none() {
        assert_eq!(in_words(0).as_deref(), Some("zero dirhams"));
        assert_eq!(in_words(1).as_deref(), Some("zero dirhams and one fil"));
        assert_eq!(in_words(100).as_deref(), Some("one dirham"));
        assert_eq!(in_words(101).as_deref(), Some("one dirham and one fil"));
        assert_eq!(
            in_words(147_000_007).as_deref(),
            Some("one million four hundred seventy thousand dirhams and seven fils"),
        );
    }

    #[test]
    fn a_figure_too_large_to_say_is_not_said_at_all() {
        // Five groups of three major units is as far as the scale words go.
        // Past that the figure is left exactly as the record wrote it rather
        // than read out with a word that does not mean what it says.
        assert!(in_words(99_999_999_999_999_999).is_some());
        assert_eq!(in_words(100_000_000_000_000_000), None);
        assert_eq!(in_words(-1), None);
    }

    #[test]
    fn what_is_said_carries_no_digits_at_all() {
        // The property that matters: a figure that was said is said in
        // words, so there is no `147000000` left anywhere in it.
        for minor in [1_i128, 100, 4_500, 147_000_000, 312_000_000, 187_200_000] {
            let said = in_words(minor).unwrap_or_else(|| panic!("{minor} can be said"));
            assert!(
                !said.contains(|character: char| character.is_ascii_digit()),
                "money is said in words, never in digits: {said}",
            );
        }
    }
}
