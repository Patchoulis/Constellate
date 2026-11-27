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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::jobs::Cancel;
    use crate::library::Declarations;
    use crate::library::papers::tests::paper;

    /// The Python's `test_filters_leave_out_what_the_settings_say_and_keep_rows_aligned`: four conference papers
    /// (an oral, two posters, a spotlight) and three from arXiv.
    fn papers() -> Vec<Paper> {
        let decided = |id: &str, decision: &str| Paper { comment: decision.to_owned(), ..paper(id) };
        vec![
            decided("x-2026-1", "Accept (oral)"),
            decided("x-2026-2", "Accept (poster)"),
            decided("x-2026-3", "Accept (spotlight)"),
            decided("x-2026-4", "Accept (poster)"),
            paper("2601.00001"),
            paper("2601.00002"),
            paper("2601.00003"),
        ]
    }

    fn read(flags: &[&str]) -> Reading {
        Reading::Read(Declarations {
            checklist: Vec::new(),
            statements: Vec::new(),
            flags: flags.iter().map(|flag| (*flag).to_owned()).collect(),
        })
    }

    /// The ids `settings` keeps, with what the library holds of declarations, and the log.
    fn kept(library: &Library, flagged: &[bool], settings: &Settings) -> (Vec<String>, Vec<String>) {
        let papers = papers();
        let accepted: Vec<Accepted> = papers.iter().map(accepted).collect();
        let declarations = declarations(library, settings).unwrap();
        let lines = Mutex::new(Vec::new());
        let log = |line: &str| lines.lock().unwrap().push(line.to_owned());
        let progress = |_| {};
        let cancel = Cancel::new();
        let tracker = Tracker::new(&log, &progress, &cancel);
        let keep = keep(&papers, &accepted, flagged, settings, &declarations, &tracker);
        assert_eq!(keep.len(), papers.len(), "one answer per paper, in their order");
        let ids = papers.iter().zip(keep).filter(|(_, kept)| *kept).map(|(paper, _)| paper.id.clone()).collect();
        (ids, lines.into_inner().unwrap())
    }

    fn library() -> (tempfile::TempDir, Library) {
        let folder = tempfile::tempdir().unwrap();
        let library = Library::open(folder.path()).unwrap();
        library.add_papers(&papers(), None, crate::library::Known::Keep).unwrap();
        let parser = crate::text::declarations::PARSER;
        library.keep_declarations(parser, &[("x-2026-3", read(&["llm_declared"])), ("x-2026-1", read(&[]))]).unwrap();
        // an older reader's reading is not trusted
        library.keep_declarations(parser - 1, &[("2601.00002", read(&["llm_declared"]))]).unwrap();
        (folder, library)
    }

    #[test]
    fn the_filters_leave_out_what_the_settings_say_and_judge_only_what_they_know() {
        let (_folder, library) = library();
        let none = [false; 7];
        let (everything, _) = kept(&library, &none, &Settings::default());
        assert_eq!(everything.len(), 7);

        let highlights = Settings { acceptance_filter: "highlights".to_owned(), ..Settings::default() };
        let (ids, log) = kept(&library, &none, &highlights);
        // arXiv papers are not judged by their acceptance
        assert_eq!(ids, ["x-2026-1", "x-2026-3", "2601.00001", "2601.00002", "2601.00003"]);
        assert!(log.iter().any(|line| line.contains("2 left out: not among: spotlights and orals")), "{log:?}");

        let strict = Settings {
            acceptance_filter: "highlights".to_owned(),
            include_ai_papers: false,
            exclude_flags: vec!["llm_declared".to_owned()],
            ..Settings::default()
        };
        let flagged = [false, true, false, false, false, false, false];
        let (ids, log) = kept(&library, &flagged, &strict);
        assert_eq!(ids, ["x-2026-1", "2601.00001", "2601.00002", "2601.00003"]);
        assert!(log.iter().any(|line| line.contains("declarations have not been read")), "{log:?}");
    }

    #[test]
    fn only_the_present_readers_declarations_are_used_and_none_are_read_when_no_flag_is_excluded() {
        let (_folder, library) = library();
        assert!(declarations(&library, &Settings::default()).unwrap().is_empty());
        let excluding = Settings { exclude_flags: vec!["llm_declared".to_owned()], ..Settings::default() };
        let mut ids: Vec<String> = declarations(&library, &excluding).unwrap().into_keys().collect();
        ids.sort();
        assert_eq!(ids, ["x-2026-1", "x-2026-3"]);
    }

    #[test]
    fn an_oral_is_kept_by_every_filter_and_a_poster_only_by_all() {
        for (filter, poster) in [("all", true), ("highlights", false), ("oral", false)] {
            let (_, tiers) = acceptance_filter(filter);
            assert!(tiers.contains(&Acceptance::Oral), "{filter}");
            assert_eq!(tiers.contains(&Acceptance::Poster), poster, "{filter}");
        }
        assert_eq!(acceptance_filter("anything else"), acceptance_filter("all"));
    }

    #[test]
    fn the_step_says_how_many_papers_go_on() {
        assert_eq!(outcome(7, 7), StepOutcome::Kept("all 7 papers go on".to_owned()));
        assert_eq!(outcome(12_487, 4), StepOutcome::Done("4 of 12,487 papers go on".to_owned()));
    }
}
