"""How a conference accepted a paper, in the conference's own words, and the tier that counts as.

Conferences name their tiers differently, and the same name changes meaning from year to year: ICLR 2023 called
its orals "notable-top-5%", CVPR calls its spotlights "Highlight", and at ICML 2021 every paper without a long talk
was a "Spotlight", which there meant an ordinary acceptance.  So a decision is not read by the words in it but
looked up in TERMS: per conference and year, every term its list was seen to use, the tier it is counted as, how
the conference writes it, and what share of the list's papers it was (docs/acceptance-tiers.md says where each row
comes from).  The engine has the same table (apps/engine/src/sources/acceptance.rs).

  the conference and year have the term     that row
  another year of the conference has it     the latest such row, said to be borrowed from that year
  no year has it                            read from its wording, said to be unknown, and collected
                                            (unknown_terms) for a maintainer to add:
                                              oral, award, best paper, outstanding, long talk  -> oral
                                              spotlight, highlight, notable                    -> spotlight
                                              anything else                                    -> poster
  no decision at all                        a poster, with no term

A decision is the list's "decision", plus the kind of any other event the list has for the same paper when the
decision does not already say so ("Accept Poster; Oral": see conferences.py), because some lists name a paper's
oral only by its talk.
"""

import re
from collections import Counter

# (conference, year, term as the list writes it, in lower case) -> (tier, the conference's wording, percentage of
# the list's papers given that wording that year, what it is there)
# fmt: off
TERMS = {
    ("neurips", 2021, "accept (oral)"): ("oral", "Oral", 2.4, "presented as a talk"),
    ("neurips", 2021, "accept (spotlight)"): ("spotlight", "Spotlight", 11.1, "a short spotlight talk"),
    ("neurips", 2021, "accept (poster)"): ("poster", "Poster", 86.5, ""),
    ("neurips", 2022, "accept; spotlight"): ("spotlight", "Featured", 19.8, "a lightning talk in a Featured Papers Panel; decisions named no tier that year"),
    ("neurips", 2022, "spotlight"): ("spotlight", "Featured", 19.8, "a lightning talk in a Featured Papers Panel; decisions named no tier that year"),
    ("neurips", 2022, "accept"): ("poster", "Accept", 78.7, "decisions named no tier that year"),
    ("neurips", 2023, "accept (oral)"): ("oral", "Oral", 2.1, "presented as a talk; main and Datasets and Benchmarks tracks"),
    ("neurips", 2023, "accept (spotlight)"): ("spotlight", "Spotlight", 11.2, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2023, "accept (poster)"): ("poster", "Poster", 86.0, ""),
    ("neurips", 2023, "accept"): ("poster", "Accept", 0.6, "ML Reproducibility Challenge reports"),
    ("neurips", 2023, "accept (outstanding paper)"): ("poster", "Outstanding paper (Reproducibility Challenge)", 0.0, "an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster"),
    ("neurips", 2023, "accept (outstanding paper honorable mention)"): ("poster", "Honorable mention (Reproducibility Challenge)", 0.0, "an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster"),
    ("neurips", 2024, "accept (oral)"): ("oral", "Oral", 1.6, "presented as a talk; main and Datasets and Benchmarks tracks"),
    ("neurips", 2024, "accept (spotlight)"): ("spotlight", "Spotlight", 8.4, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2024, "accept (poster)"): ("poster", "Poster", 89.6, ""),
    ("neurips", 2025, "accept (oral)"): ("oral", "Oral", 1.6, "presented as a talk; main, Datasets and Benchmarks, and position paper tracks"),
    ("neurips", 2025, "accept (spotlight)"): ("spotlight", "Spotlight", 12.7, "main and Datasets and Benchmarks tracks"),
    ("neurips", 2025, "accept (poster)"): ("poster", "Poster", 84.9, ""),
    ("neurips", 2025, "accept"): ("poster", "Accept", 0.5, "position paper track"),
    ("neurips", 2026, "accept (oral)"): ("oral", "Oral", 1.5, "presented as a talk; main, Evaluations and Datasets, and position paper tracks"),
    ("neurips", 2026, "accept (spotlight)"): ("spotlight", "Spotlight", 3.7, "main, Evaluations and Datasets, and position paper tracks"),
    ("neurips", 2026, "accept (poster)"): ("poster", "Poster", 92.4, ""),
    ("neurips", 2026, "accept"): ("poster", "Accept", 1.3, "position paper track"),
    ("icml", 2021, "oral"): ("oral", "Oral", 14.0, "a long talk"),
    ("icml", 2021, "spotlight"): ("poster", "Spotlight", 86.0, "in 2021 every paper without a long talk gave a short spotlight talk: not singled out"),
    ("icml", 2022, "accept for long presentation; oral"): ("oral", "Long presentation", 9.6, "a long talk"),
    ("icml", 2022, "accept for short presentation; spotlight"): ("poster", "Short presentation", 90.4, "every other accepted paper gave a short talk: not singled out"),
    ("icml", 2023, "accept (oral & poster)"): ("oral", "Oral", 8.3, "presented as a talk and a poster"),
    ("icml", 2023, "accept (poster)"): ("poster", "Poster", 91.7, ""),
    ("icml", 2024, "accept (oral)"): ("oral", "Oral", 5.5, "presented as a talk"),
    ("icml", 2024, "accept (spotlight)"): ("spotlight", "Spotlight", 7.3, "a spotlight poster"),
    ("icml", 2024, "accept (poster)"): ("poster", "Poster", 87.3, ""),
    ("icml", 2025, "accept (oral)"): ("oral", "Oral", 3.6, "presented as a talk"),
    ("icml", 2025, "accept (spotlight poster)"): ("spotlight", "Spotlight poster", 6.7, ""),
    ("icml", 2025, "accept (poster)"): ("poster", "Poster", 89.7, ""),
    ("icml", 2026, "accept (spotlight); oral"): ("oral", "Oral", 2.5, "chosen for a talk from among the spotlights"),
    ("icml", 2026, "oral"): ("oral", "Oral", 0.0, "a talk listed without a decision"),
    ("icml", 2026, "accept (spotlight)"): ("spotlight", "Spotlight", 6.1, "main and position paper tracks"),
    ("icml", 2026, "accept (regular)"): ("poster", "Regular", 90.3, ""),
    ("icml", 2026, "accept (poster)"): ("poster", "Poster", 0.1, "journal-track and other invited posters"),
    ("iclr", 2021, "oral"): ("oral", "Oral", 6.2, "presented as a talk"),
    ("iclr", 2021, "spotlight"): ("spotlight", "Spotlight", 13.3, "a short spotlight talk"),
    ("iclr", 2021, "poster"): ("poster", "Poster", 80.6, ""),
    ("iclr", 2022, "accept (oral)"): ("oral", "Oral", 5.0, "presented as a talk"),
    ("iclr", 2022, "accept (spotlight)"): ("spotlight", "Spotlight", 15.9, "a short spotlight talk"),
    ("iclr", 2022, "accept (poster)"): ("poster", "Poster", 79.1, ""),
    ("iclr", 2023, "accept: notable-top-5%; oral"): ("oral", "Notable top 5%", 5.7, "the conference's name that year for its orals"),
    ("iclr", 2023, "accept: notable-top-5%"): ("oral", "Notable top 5%", 5.7, "the conference's name that year for its orals"),
    ("iclr", 2023, "accept: notable-top-25%; oral"): ("spotlight", "Notable top 25%", 17.7, "the conference's name that year for its spotlights"),
    ("iclr", 2023, "accept: notable-top-25%"): ("spotlight", "Notable top 25%", 17.7, "the conference's name that year for its spotlights"),
    ("iclr", 2023, "accept: poster"): ("poster", "Poster", 75.9, ""),
    ("iclr", 2023, "accept (poster)"): ("poster", "Poster", 0.6, "journal-track and other invited posters"),
    ("iclr", 2024, "accept (oral)"): ("oral", "Oral", 3.7, "presented as a talk"),
    ("iclr", 2024, "accept (spotlight)"): ("spotlight", "Spotlight", 16.0, "a spotlight poster"),
    ("iclr", 2024, "accept (poster)"): ("poster", "Poster", 80.3, ""),
    ("iclr", 2025, "accept (oral)"): ("oral", "Oral", 5.6, "presented as a talk"),
    ("iclr", 2025, "accept (spotlight)"): ("spotlight", "Spotlight", 9.9, ""),
    ("iclr", 2025, "accept (poster)"): ("poster", "Poster", 83.2, ""),
    ("iclr", 2025, "accept"): ("poster", "Blog post", 1.3, "the blog post track"),
    ("iclr", 2026, "accept (oral)"): ("oral", "Oral", 4.1, "presented as a talk; there was no spotlight tier that year"),
    ("iclr", 2026, "accept (poster)"): ("poster", "Poster", 95.9, ""),
    ("aistats", 2021, "oral"): ("oral", "Oral", 10.5, "presented as a talk"),
    ("aistats", 2021, "poster"): ("poster", "Poster", 89.5, ""),
    ("aistats", 2022, "accept (oral)"): ("oral", "Oral", 8.9, "presented as a talk"),
    ("aistats", 2022, "accept (poster)"): ("poster", "Poster", 91.1, ""),
    ("aistats", 2023, "accept (oral)"): ("oral", "Oral", 6.5, "presented as a talk"),
    ("aistats", 2023, "accept (poster)"): ("poster", "Poster", 93.5, ""),
    ("aistats", 2024, "accept (oral)"): ("oral", "Oral", 8.6, "presented as a talk"),
    ("aistats", 2024, "accept (poster)"): ("poster", "Poster", 91.4, ""),
    ("aistats", 2025, "accept (oral)"): ("oral", "Oral", 6.3, "presented as a talk"),
    ("aistats", 2025, "accept (poster)"): ("poster", "Poster", 92.4, ""),
    ("aistats", 2026, "accept (poster); oral"): ("oral", "Oral", 1.4, "presented as a talk"),
    ("aistats", 2026, "accept (poster)"): ("poster", "Poster", 98.6, ""),
    ("mlsys", 2021, "accept; oral"): ("poster", "Accept", 100.0, "every MLSys paper gives a talk: not singled out"),
    ("mlsys", 2022, "accept; oral"): ("poster", "Accept", 100.0, "every MLSys paper gives a talk: not singled out"),
    ("mlsys", 2023, "accept"): ("poster", "Accept", 100.0, ""),
    ("mlsys", 2024, "accept"): ("poster", "Accept", 89.2, ""),
    ("mlsys", 2024, "conditional accept"): ("poster", "Conditional accept", 10.8, ""),
    ("mlsys", 2025, "accept"): ("poster", "Accept", 78.7, ""),
    ("mlsys", 2025, "accept with shepherding"): ("poster", "Accept with shepherding", 21.3, ""),
    ("mlsys", 2026, "conditional accept; oral"): ("poster", "Conditional accept", 78.9, "every MLSys paper gives a talk: not singled out"),
    ("cvpr", 2023, "accept"): ("poster", "Accept", 100.0, "the list does not name 2023's highlights or orals"),
    ("cvpr", 2024, "accept: oral"): ("oral", "Oral", 3.3, "presented as a talk"),
    ("cvpr", 2024, "accept: poster (highlight)"): ("spotlight", "Highlight", 11.9, "a poster singled out as a highlight"),
    ("cvpr", 2024, "accept: poster"): ("poster", "Poster", 84.8, ""),
    ("cvpr", 2025, "oral"): ("oral", "Oral", 3.3, "presented as a talk; the list does not name 2025's highlights"),
    ("cvpr", 2026, "accept (oral)"): ("oral", "Oral", 4.0, "presented as a talk"),
    ("cvpr", 2026, "accept (highlight)"): ("spotlight", "Highlight", 9.0, "a poster singled out as a highlight"),
    ("cvpr", 2026, "accept (poster)"): ("poster", "Poster", 45.7, ""),
    ("iccv", 2025, "oral"): ("oral", "Oral", 2.4, "presented as a talk"),
    ("iccv", 2025, "highlight"): ("spotlight", "Highlight", 9.7, "a poster singled out as a highlight"),
    ("iccv", 2025, "poster"): ("poster", "Poster", 87.9, ""),
    ("eccv", 2024, "oral"): ("oral", "Oral", 8.4, "presented as a talk"),
    ("eccv", 2026, "accept poster; oral"): ("oral", "Oral", 1.0, "presented as a talk"),
    ("eccv", 2026, "accept poster; spotlight"): ("spotlight", "Spotlight", 4.8, "a short spotlight talk"),
    ("eccv", 2026, "accept poster"): ("poster", "Poster", 94.2, ""),
    ("wacv", 2026, "accept poster; oral"): ("oral", "Oral", 10.2, "presented as a talk"),
    ("wacv", 2026, "accept poster"): ("poster", "Poster", 89.8, ""),
}
# fmt: on

# A word that puts a term never seen before in a tier, best first; anything else is a poster.
WORDING = (
    ("oral", ("oral", "award", "best paper", "outstanding", "long talk", "long presentation")),
    ("spotlight", ("spotlight", "highlight", "notable")),
)

RANK = {"oral": 0, "spotlight": 1, "poster": 2}  # best first

UNKNOWN = "not yet in Constellate's table of this conference's terms, so its tier was read from its wording"


def _name(slug: str) -> str:
    """The conference's name as the catalogue writes it ("neurips" -> "NeurIPS")."""
    from .catalog import CONFERENCES  # here, not at the top: the catalogue imports modules that import this one

    names = {name.lower(): name for venues in CONFERENCES.values() for name, _ in venues}
    return names.get(slug, slug.upper())


def _share(percent: float) -> str:
    return "under 0.1% of the papers in its list" if percent < 0.1 else f"{percent:.1f}% of the papers in its list"


def _as_written(decision: str) -> str:
    """A term never seen, as it is written without "Accept": "Accept (Best Paper)" -> "Best Paper"."""
    rest = re.sub(r"(?i)^accept(?![^\W\d_])", "", decision).strip(" \t():;-")
    return rest or decision.strip()


def read(slug: str, year: str, decision: str) -> tuple[str, dict | None]:
    """(tier, term) for a decision of a conference's list in a year; the term is AcceptanceTerm in
    packages/contracts, or None when the list gives no decision."""
    decision = (decision or "").strip()
    if not decision:
        return "poster", None
    conference, term = f"{_name(slug)} {year}", decision.lower()
    found = TERMS.get((slug, int(year), term))
    borrowed = None
    if found is None:
        years = [y for (s, y, t) in TERMS if s == slug and t == term]
        if years:
            borrowed = max(years)
            found = TERMS[(slug, borrowed, term)]
    worded = next((tier for tier, words in WORDING if any(word in term for word in words)), "poster")
    if found is not None:
        tier, called, percent, what = found
        means = _share(percent) + (f"; {what}" if what else "")
        if borrowed:
            means = f"read as at {_name(slug)} {borrowed}, where it was {means}"
        # A borrowed row that ranks the term below what its words say (ICML 2021's "Spotlight", NeurIPS 2023's
        # Reproducibility Challenge award) may have meant that only that year: keep its lower tier, never promote,
        # but collect it for a maintainer to confirm.
        known = not borrowed or RANK[worded] >= RANK[tier]
        return tier, {"conference": conference, "called": called, "means": means, "known": known}
    return worded, {"conference": conference, "called": _as_written(decision), "means": UNKNOWN, "known": False}


def equivalents(tier: str) -> list[str]:
    """The terms of every conference counted as this tier, as the conferences write them, each once."""
    called = []
    for row_tier, name, _, _ in TERMS.values():
        if row_tier == tier and name.lower() not in (known.lower() for known in called):
            called.append(name)
    return called


def unknown_terms(papers, read_paper) -> Counter:
    """(conference, decision as written) -> how many of these papers have a decision no row knows."""
    unknown = Counter()
    for paper in papers:
        _, term = read_paper(paper)
        if term and not term["known"]:
            unknown[(term["conference"], paper.get("comment", "").strip())] += 1
    return unknown
