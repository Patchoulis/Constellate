//! The filter step: what the settings say to leave out, left out. Each rule judges only the papers it knows
//! about: the acceptance rule conference papers, the declarations rule papers whose declarations have been read,
//! the AI-text rule papers the detector flagged.

use std::collections::{HashMap, HashSet};

use super::steps::{StepOutcome, thousands};
use crate::jobs::Tracker;
use crate::library::{Library, LibraryError, Paper, Reading};
use crate::settings::Settings;
use crate::sources::acceptance::{self, Acceptance, Accepted};

/// The tiers an acceptance filter lets through, and how it is called (the Python's `settings.ACCEPTANCE`).
pub fn acceptance_filter(name: &str) -> (&'static str, &'static [Acceptance]) {
    match name {
        "highlights" => ("Spotlights and orals", &[Acceptance::Oral, Acceptance::Spotlight]),
        "oral" => ("Orals only", &[Acceptance::Oral]),
        _ => ("All accepted papers", &[Acceptance::Oral, Acceptance::Spotlight, Acceptance::Poster]),
    }
}

/// How a paper of the library was accepted: read from its decision when it came from a conference's list.
pub fn accepted(paper: &Paper) -> Accepted {
    if paper.origin() == crate::library::Origin::Conference {
        let mut parts = paper.id.splitn(3, '-');
        if let (Some(slug), Some(year)) = (parts.next(), parts.next()) {
            return acceptance::read(slug, year, &paper.comment);
        }
    }
    Accepted { tier: Acceptance::NotListed, term: None }
}

/// Which papers stay, and the log's lines on what was left out and why.
pub fn keep(
    papers: &[Paper],
    accepted: &[Accepted],
    flagged: &[bool],
    settings: &Settings,
    declarations: &HashMap<String, Reading>,
    tracker: &Tracker<'_>,
) -> Vec<bool> {
    let mut keep = vec![true; papers.len()];
    let mut leave_out = |drop: &dyn Fn(usize) -> bool, why: &str| {
        let mut dropped = 0;
        for (index, kept) in keep.iter_mut().enumerate() {
            if *kept && drop(index) {
                dropped += 1;
            }
            if drop(index) {
                *kept = false;
            }
        }
        if dropped > 0 {
            tracker.log(&format!("  {dropped} left out: {why}"));
        }
    };
    if !settings.include_ai_papers {
        leave_out(&|index| flagged[index], "tagged as written the way language models write");
    }
    let (label, tiers) = acceptance_filter(&settings.acceptance_filter);
    let mut unknown: Vec<((String, String), usize)> = Vec::new();
    for (paper, shown) in papers.iter().zip(accepted) {
        if let Some(term) = shown.term.as_ref().filter(|term| !term.known) {
            let key = (term.conference.clone(), paper.comment.trim().to_owned());
            match unknown.iter_mut().find(|(known, _)| *known == key) {
                Some((_, count)) => *count += 1,
                None => unknown.push((key, 1)),
            }
        }
    }
    unknown.sort_by_key(|(_, count)| std::cmp::Reverse(*count)); // most common first, as the Python's Counter (stable)
    for ((conference, decision), count) in unknown {
        tracker.log(&format!(
            "  NOTE: {count} {conference} papers have a decision Constellate's table of tiers does not know, \
             {decision:?}: its tier was read from its wording (add it to sources/acceptance.rs)"
        ));
    }
    leave_out(
        &|index| accepted[index].tier != Acceptance::NotListed && !tiers.contains(&accepted[index].tier),
        &format!("not among: {}", label.to_lowercase()),
    );
    if !settings.exclude_flags.is_empty() {
        let excluded: HashSet<&str> = settings.exclude_flags.iter().map(String::as_str).collect();
        let raises = |index: usize| match declarations.get(&papers[index].id) {
            Some(Reading::Read(read)) => read.flags.iter().any(|flag| excluded.contains(flag.as_str())),
            Some(Reading::NoPdf | Reading::Unreadable) | None => false,
        };
        leave_out(&raises, "their authors' declarations raise an excluded flag");
        let unread = papers.iter().filter(|paper| !declarations.contains_key(&paper.id)).count();
        if unread > 0 {
            tracker.log(&format!(
                "  NOTE: {unread} of {} papers' declarations have not been read, so that rule could not judge them \
                 and they stay in (Settings: Read them all)",
                papers.len()
            ));
        }
    }
    keep
}

/// The declarations read so far, when a rule needs them.
pub fn declarations(library: &Library, settings: &Settings) -> Result<HashMap<String, Reading>, LibraryError> {
    if settings.exclude_flags.is_empty() {
        return Ok(HashMap::new());
    }
    library.all_declarations(crate::text::declarations::PARSER)
}

/// What the step came to.
pub fn outcome(before: usize, after: usize) -> StepOutcome {
    if before == after {
        StepOutcome::Kept(format!("all {} papers go on", thousands(after as u64)))
    } else {
        StepOutcome::Done(format!("{} of {} papers go on", thousands(after as u64), thousands(before as u64)))
    }
}
