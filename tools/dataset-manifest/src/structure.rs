//! Structural checks over a set of signed entries: the defects no per-entry
//! check can see, because they are properties of the set itself.
//!
//! Shared by signing — which refuses to write a manifest carrying any of them —
//! and verification — which refuses to call such a manifest clean — so the
//! writer and the reader cannot drift apart on what a usable set is.

use std::collections::BTreeMap;

use crate::project::PROJECT_FILE;

/// Everything wrong with a set of `(doc_id, path)` entries, as human-readable
/// lines. `what` names the file those lines are about, because the same rule
/// is run over `project.json`'s `documents` and over `manifest.json`'s.
pub(crate) fn problems(entries: &[(&str, &str)], what: &str) -> Vec<String> {
    let mut problems = duplicates(entries.iter().map(|(doc_id, _)| *doc_id), what, "doc_id");
    problems.extend(duplicates(
        entries.iter().map(|(_, path)| *path),
        what,
        "path",
    ));
    if !entries.iter().any(|(_, path)| *path == PROJECT_FILE) {
        problems.push(format!(
            "{what} does not cover {PROJECT_FILE} itself, the record that states the \
             milestones, the payment schedules and the escrow balance; declare it among \
             the dataset's \"documents\" with \"path\": \"{PROJECT_FILE}\" and re-sign, or \
             every one of those facts is covered by no signature at all"
        ));
    }
    problems
}

/// Every value that appears more than once, named with the number of times.
///
/// All of them, not the first: an operator who fixes one duplicate and re-runs
/// should not discover the next one a run at a time.
pub(crate) fn duplicates<'a>(
    values: impl Iterator<Item = &'a str>,
    what: &str,
    field: &str,
) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(value, count)| {
            format!(
                "{what} lists {field} {value:?} {count} times; a lookup here takes the first \
                 entry while a consumer that builds a map from the same file keeps the last, \
                 so the two would disagree about what is signed"
            )
        })
        .collect()
}
