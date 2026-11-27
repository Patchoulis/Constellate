"""How each conference names its tiers, read from the decisions its own lists were seen to use (rp/tiers.py)."""

import pytest

from conftest import paper
from rp import graph, tiers
from rp.library import acceptance, acceptance_term, accepted

# Every decision seen in the lists (docs/acceptance-tiers.md), as the list writes it, with the tier and the
# wording it must be shown with.
SEEN = [
    ("neurips-2021-1", "Accept (Oral)", "oral", "Oral"),
    ("neurips-2021-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("neurips-2021-1", "Accept (Poster)", "poster", "Poster"),
    ("neurips-2022-1", "Accept; Spotlight", "spotlight", "Featured"),
    ("neurips-2022-1", "Spotlight", "spotlight", "Featured"),
    ("neurips-2022-1", "Accept", "poster", "Accept"),
    ("neurips-2023-1", "Accept (oral)", "oral", "Oral"),
    ("neurips-2023-1", "Accept (Oral)", "oral", "Oral"),
    ("neurips-2023-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("neurips-2023-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("neurips-2023-1", "Accept (poster)", "poster", "Poster"),
    ("neurips-2023-1", "Accept (Poster)", "poster", "Poster"),
    ("neurips-2023-1", "Accept", "poster", "Accept"),
    ("neurips-2023-1", "Accept (Outstanding Paper)", "poster", "Outstanding paper (Reproducibility Challenge)"),
    (
        "neurips-2023-1",
        "Accept (Outstanding Paper Honorable Mention)",
        "poster",
        "Honorable mention (Reproducibility Challenge)",
    ),
    ("neurips-2024-1", "Accept (oral)", "oral", "Oral"),
    ("neurips-2024-1", "Accept (Oral)", "oral", "Oral"),
    ("neurips-2024-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("neurips-2024-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("neurips-2024-1", "Accept (poster)", "poster", "Poster"),
    ("neurips-2024-1", "Accept (Poster)", "poster", "Poster"),
    ("neurips-2025-1", "Accept (oral)", "oral", "Oral"),
    ("neurips-2025-1", "Accept (Oral)", "oral", "Oral"),
    ("neurips-2025-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("neurips-2025-1", "Accept (poster)", "poster", "Poster"),
    ("neurips-2025-1", "Accept (Poster)", "poster", "Poster"),
    ("neurips-2025-1", "Accept", "poster", "Accept"),
    ("neurips-2026-1", "Accept (oral)", "oral", "Oral"),
    ("neurips-2026-1", "Accept (Oral)", "oral", "Oral"),
    ("neurips-2026-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("neurips-2026-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("neurips-2026-1", "Accept (poster)", "poster", "Poster"),
    ("neurips-2026-1", "Accept", "poster", "Accept"),
    ("icml-2021-1", "Oral", "oral", "Oral"),
    ("icml-2021-1", "Spotlight", "poster", "Spotlight"),
    ("icml-2022-1", "Accept for Long Presentation; Oral", "oral", "Long presentation"),
    ("icml-2022-1", "Accept for Short Presentation; Spotlight", "poster", "Short presentation"),
    ("icml-2023-1", "Accept (Oral & Poster)", "oral", "Oral"),
    ("icml-2023-1", "Accept (Poster)", "poster", "Poster"),
    ("icml-2024-1", "Accept (Oral)", "oral", "Oral"),
    ("icml-2024-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("icml-2024-1", "Accept (Poster)", "poster", "Poster"),
    ("icml-2025-1", "Accept (oral)", "oral", "Oral"),
    ("icml-2025-1", "Accept (spotlight poster)", "spotlight", "Spotlight poster"),
    ("icml-2025-1", "Accept (poster)", "poster", "Poster"),
    ("icml-2025-1", "Accept (Poster)", "poster", "Poster"),
    ("icml-2026-1", "Accept (spotlight); Oral", "oral", "Oral"),
    ("icml-2026-1", "Oral", "oral", "Oral"),
    ("icml-2026-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("icml-2026-1", "Accept (regular)", "poster", "Regular"),
    ("icml-2026-1", "Accept (Poster)", "poster", "Poster"),
    ("iclr-2021-1", "Oral", "oral", "Oral"),
    ("iclr-2021-1", "Spotlight", "spotlight", "Spotlight"),
    ("iclr-2021-1", "Poster", "poster", "Poster"),
    ("iclr-2022-1", "Accept (Oral)", "oral", "Oral"),
    ("iclr-2022-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("iclr-2022-1", "Accept (Poster)", "poster", "Poster"),
    ("iclr-2023-1", "Accept: notable-top-5%; Oral", "oral", "Notable top 5%"),
    ("iclr-2023-1", "Accept: notable-top-5%", "oral", "Notable top 5%"),
    ("iclr-2023-1", "Accept: notable-top-25%; Oral", "spotlight", "Notable top 25%"),
    ("iclr-2023-1", "Accept: notable-top-25%", "spotlight", "Notable top 25%"),
    ("iclr-2023-1", "Accept: poster", "poster", "Poster"),
    ("iclr-2023-1", "Accept (Poster)", "poster", "Poster"),
    ("iclr-2024-1", "Accept (oral)", "oral", "Oral"),
    ("iclr-2024-1", "Accept (spotlight)", "spotlight", "Spotlight"),
    ("iclr-2024-1", "Accept (poster)", "poster", "Poster"),
    ("iclr-2024-1", "Accept (Poster)", "poster", "Poster"),
    ("iclr-2025-1", "Accept (Oral)", "oral", "Oral"),
    ("iclr-2025-1", "Accept (Spotlight)", "spotlight", "Spotlight"),
    ("iclr-2025-1", "Accept (Poster)", "poster", "Poster"),
    ("iclr-2025-1", "Accept", "poster", "Blog post"),
    ("iclr-2026-1", "Accept (Oral)", "oral", "Oral"),
    ("iclr-2026-1", "Accept (Poster)", "poster", "Poster"),
    ("aistats-2021-1", "Oral", "oral", "Oral"),
    ("aistats-2021-1", "Poster", "poster", "Poster"),
    ("aistats-2022-1", "Accept (Oral)", "oral", "Oral"),
    ("aistats-2022-1", "Accept (Poster)", "poster", "Poster"),
    ("aistats-2023-1", "Accept (Oral)", "oral", "Oral"),
    ("aistats-2023-1", "Accept (Poster)", "poster", "Poster"),
    ("aistats-2024-1", "Accept (Oral)", "oral", "Oral"),
    ("aistats-2024-1", "Accept (Poster)", "poster", "Poster"),
    ("aistats-2025-1", "Accept (Oral)", "oral", "Oral"),
    ("aistats-2025-1", "Accept (Poster)", "poster", "Poster"),
    ("aistats-2026-1", "Accept (Poster); Oral", "oral", "Oral"),
    ("aistats-2026-1", "Accept (Poster)", "poster", "Poster"),
    ("mlsys-2021-1", "Accept; Oral", "poster", "Accept"),
    ("mlsys-2022-1", "Accept; Oral", "poster", "Accept"),
    ("mlsys-2023-1", "Accept", "poster", "Accept"),
    ("mlsys-2024-1", "Accept", "poster", "Accept"),
    ("mlsys-2024-1", "Conditional Accept", "poster", "Conditional accept"),
    ("mlsys-2025-1", "Accept", "poster", "Accept"),
    ("mlsys-2025-1", "Accept with shepherding", "poster", "Accept with shepherding"),
    ("mlsys-2026-1", "Conditional Accept; Oral", "poster", "Conditional accept"),
    ("cvpr-2023-1", "Accept", "poster", "Accept"),
    ("cvpr-2024-1", "Accept: Oral", "oral", "Oral"),
    ("cvpr-2024-1", "Accept: Poster (Highlight)", "spotlight", "Highlight"),
    ("cvpr-2024-1", "Accept: Poster", "poster", "Poster"),
    ("cvpr-2025-1", "Oral", "oral", "Oral"),
    ("cvpr-2026-1", "Accept (Oral)", "oral", "Oral"),
    ("cvpr-2026-1", "Accept (Highlight)", "spotlight", "Highlight"),
    ("cvpr-2026-1", "Accept (Poster)", "poster", "Poster"),
    ("iccv-2025-1", "oral", "oral", "Oral"),
    ("iccv-2025-1", "highlight", "spotlight", "Highlight"),
    ("iccv-2025-1", "poster", "poster", "Poster"),
    ("eccv-2024-1", "Oral", "oral", "Oral"),
    ("eccv-2026-1", "Accept Poster; Oral", "oral", "Oral"),
    ("eccv-2026-1", "Accept Poster; Spotlight", "spotlight", "Spotlight"),
    ("eccv-2026-1", "Accept Poster", "poster", "Poster"),
    ("wacv-2026-1", "Accept Poster; Oral", "oral", "Oral"),
    ("wacv-2026-1", "Accept Poster", "poster", "Poster"),
]


@pytest.mark.parametrize("paper_id, decision, tier, called", SEEN)
def test_every_decision_seen_in_the_lists_is_read_as_its_conference_meant_it(paper_id, decision, tier, called):
    found_tier, term = accepted(paper(paper_id, comment=decision))
    assert (found_tier, term["called"], term["known"]) == (tier, called, True)


def test_the_same_word_means_different_tiers_at_different_conferences_and_years():
    # "Spotlight" was ICML 2021's ordinary acceptance, and ICLR 2021's spotlight
    assert acceptance(paper("icml-2021-1", comment="Spotlight")) == "poster"
    assert acceptance(paper("iclr-2021-1", comment="Spotlight")) == "spotlight"
    # ICML 2026 named its orals only by their talk; MLSys gives every paper one
    assert acceptance(paper("icml-2026-1", comment="Accept (spotlight); Oral")) == "oral"
    assert acceptance(paper("mlsys-2026-1", comment="Conditional Accept; Oral")) == "poster"


def test_the_hover_text_says_what_share_the_term_was_and_what_it_is():
    assert acceptance_term(paper("cvpr-2024-7", comment="Accept: Poster (Highlight)")) == {
        "conference": "CVPR 2024",
        "called": "Highlight",
        "means": "11.9% of the papers in its list; a poster singled out as a highlight",
        "known": True,
    }


def test_a_year_not_in_the_table_borrows_the_latest_year_that_used_the_term():
    tier, term = accepted(paper("neurips-2027-1", comment="Accept (oral)"))
    assert tier == "oral" and term["known"] and term["conference"] == "NeurIPS 2027"
    assert term["means"].startswith("read as at NeurIPS 2026, where it was 1.5% of the papers")
    # a borrowed row lower than the term's words is kept, not promoted, and collected to be confirmed
    for paper_id, decision in [("icml-2027-1", "Spotlight"), ("neurips-2027-1", "Accept (Outstanding Paper)")]:
        tier, term = accepted(paper(paper_id, comment=decision))
        assert (tier, term["known"]) == ("poster", False), decision
    assert acceptance_term(paper("mlsys-2027-1", comment="Accept"))["known"]


@pytest.mark.parametrize(
    "decision, tier, called",
    [
        ("Accept (Best Paper)", "oral", "Best Paper"),
        ("Award Candidate", "oral", "Award Candidate"),
        ("Accept: Outstanding Paper Award", "oral", "Outstanding Paper Award"),
        ("Accept (Highlight Paper)", "spotlight", "Highlight Paper"),
        ("Accept (regular)", "poster", "regular"),
        ("Accepted", "poster", "Accepted"),
        ("Weird new thing", "poster", "Weird new thing"),
    ],
)
def test_a_term_never_seen_is_read_from_its_wording_and_said_to_be_unknown(decision, tier, called):
    found_tier, term = accepted(paper("cvpr-2027-1", comment=decision))
    assert (found_tier, term["called"], term["known"]) == (tier, called, False)
    assert term["means"] == tiers.UNKNOWN


def test_terms_never_seen_are_collected_for_a_maintainer():
    papers = [
        paper("cvpr-2027-1", comment="Award Candidate"),
        paper("cvpr-2027-2", comment="Award Candidate"),
        paper("cvpr-2024-3", comment="Accept: Oral"),
        paper("2512.01234", comment="Accepted as an oral"),
    ]
    assert tiers.unknown_terms(papers, accepted) == {("CVPR 2027", "Award Candidate"): 2}


def test_no_decision_is_a_poster_with_no_term_and_an_arxiv_paper_has_neither():
    assert accepted(paper("eccv-2024-1", comment="")) == ("poster", None)
    assert accepted(paper("2512.01234", comment="Accepted as an oral at NeurIPS 2025")) == (None, None)


def test_each_tier_lists_the_terms_it_includes():
    spotlights = tiers.equivalents("spotlight")
    assert {"Spotlight", "Highlight", "Notable top 25%", "Spotlight poster", "Featured"} <= set(spotlights)
    assert "Long presentation" in tiers.equivalents("oral") and "Oral" not in spotlights


def test_a_graph_node_carries_the_term_only_when_it_gets_a_tag():
    assert graph._acceptance(paper("iccv-2025-1", comment="highlight")) == {
        "acceptance": "spotlight",
        "acceptance_term": {
            "conference": "ICCV 2025",
            "called": "Highlight",
            "means": "9.7% of the papers in its list; a poster singled out as a highlight",
            "known": True,
        },
    }
    assert graph._acceptance(paper("iccv-2025-2", comment="poster")) == {"acceptance": "poster"}
    assert graph._acceptance(paper("2512.01234")) == {"acceptance": None}


def test_the_table_has_shares_that_are_percentages_and_terms_in_lower_case():
    assert all(0 <= share <= 100 for _, _, share, _ in tiers.TERMS.values())
    assert all(term == term.lower() for _, _, term in tiers.TERMS)
    assert {tier for tier, *_ in tiers.TERMS.values()} == {"oral", "spotlight", "poster"}
