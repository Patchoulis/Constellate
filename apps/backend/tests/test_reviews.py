"""Reviewers' scores, read from the text of a reviews page as the reader shows it."""

from conftest import paper
from rp import reviews, server

FORUM = "https://openreview.net/forum?id=abc123XYZ"

NEURIPS = """
Paper Title
Official Review of Submission1234 by Reviewer aBcD
Summary:
The paper proposes a method. Quality: it is fine overall.
Strengths And Weaknesses:
Quality: 2 of the 3 proofs are sketched only. Clarity: 3/5 I would say.
Quality: 2: fair
Clarity: 2: fair
Significance: 3: good
Originality: 4: excellent
Rating: 4: Borderline accept
Confidence: 3: You are fairly confident in your assessment.

Official Comment by Authors
We thank the reviewer. To recall, the review said:
Quality: 2: fair
Rating: 4: Borderline accept

Official Comment by Reviewer aBcD
Thanks, I raise my score. Rating: 4 -> 5

Official Review of Submission1234 by Reviewer eFgH
Quality:
3: good
Clarity: 4: excellent
Significance: 3: good Originality: 3: good
Rating: 5: Accept
Confidence: 4

Meta Review of Submission1234 by Area Chair XyZw
Recommendation: Accept
Confidence: 5: Certain
"""

ICLR = """
Official Review of Submission77 by Reviewer One
Soundness: 3: good
Presentation: 2: fair
Contribution: 3: good
Rating: 6: marginally above the acceptance threshold
Confidence: 4: You are confident in your assessment
Official Review of Submission77 by Reviewer Two
soundness: 4 excellent
Presentation: 3 good
Contribution: 2 fair
Rating: 8: accept, good paper
"""

NO_HEADINGS = "Soundness: 3: good\nRating: 6: weak accept\nSoundness: 4: excellent\nRating: 8: accept\n"


def test_only_reviews_are_read_one_score_per_field_each():
    read = reviews.read_ratings(NEURIPS)
    assert read["reviews"] == 2 and read["exact"] is True
    assert list(read["ratings"]) == ["Rating", "Quality", "Clarity", "Significance", "Originality", "Confidence"]
    assert read["ratings"]["Quality"]["values"] == [2, 3]  # not the prose, not the authors' quote
    assert read["ratings"]["Clarity"] == {"values": [2, 4], "meanings": ["fair", "excellent"], "mean": 3.0}
    assert read["ratings"]["Rating"]["values"] == [4, 5]  # not the comment's "4 -> 5"
    assert read["ratings"]["Rating"]["meanings"][0] == "Borderline accept"
    assert read["ratings"]["Confidence"]["values"] == [3, 4]  # not the meta-review's
    assert read["ratings"]["Significance"]["meanings"] == ["good", "good"]  # two fields on one line
    assert read["ratings"]["Originality"] == {"values": [4, 3], "meanings": ["excellent", "good"], "mean": 3.5}


def test_another_conferences_fields_and_ways_of_writing_them():
    read = reviews.read_ratings(ICLR)
    assert read["ratings"]["Soundness"] == {"values": [3, 4], "meanings": ["good", "excellent"], "mean": 3.5}
    assert read["ratings"]["Rating"]["mean"] == 7.0
    assert read["reviews"] == 2


def test_scores_that_are_not_whole_scores_are_left_alone():
    for line in (
        "Rating: 6.5",
        "Clarity: 3/5",
        "Originality: 2 out of 4",
        "Rating: 5 -> 7",
        "Quality: 2 of the proofs",
    ):
        assert reviews.read_ratings(f"Official Review of Submission1 by Reviewer A\n{line}\n") is None, line


def test_a_page_without_review_headings_is_read_whole_and_says_it_is_a_guess():
    read = reviews.read_ratings(NO_HEADINGS)
    assert read["exact"] is False and read["reviews"] == 2
    assert read["ratings"]["Rating"]["values"] == [6, 8]


def test_a_page_without_scores_says_nothing_and_changes_nothing():
    assert reviews.read_ratings("Just a moment... checking your browser.") is None
    assert reviews.read_ratings("") is None
    one = paper("x-2026-1", url=FORUM)
    first = reviews.keep(one, ICLR, FORUM)
    assert first["reviews"] == 2
    assert reviews.keep(one, "Loading", FORUM) == first == reviews.known("x-2026-1")
    assert reviews.known("x-2026-404") is None


def test_a_fuller_reading_replaces_a_partial_one_but_not_the_other_way_round():
    one = paper("x-2026-2", url=FORUM)
    half = ICLR[: ICLR.index("Official Review of Submission77 by Reviewer Two")]
    assert reviews.keep(one, half, FORUM)["reviews"] == 1  # the page was still loading
    assert reviews.keep(one, ICLR, FORUM)["reviews"] == 2
    assert reviews.keep(one, half, FORUM)["reviews"] == 2  # collapsed replies later do not undo it
    assert reviews.keep(one, NO_HEADINGS, FORUM)["exact"] is True  # nor does a vaguer reading


def test_another_papers_page_is_not_read_as_this_papers():
    one = paper("x-2026-3", url=FORUM)
    for elsewhere in (
        "https://openreview.net/forum?id=SOMEONEELSE",
        "https://openreview.net/group?id=NeurIPS.cc",
        "",
        "https://example.org/?id=abc123XYZ",
    ):
        assert reviews.keep(one, ICLR, elsewhere) is None
    assert reviews.keep(paper("2601.00001", url="https://arxiv.org/abs/2601.00001"), ICLR, FORUM) is None
    assert reviews.keep(one, ICLR, FORUM + "&noteId=xyz")["reviews"] == 2
    assert reviews.keep(one, ICLR, "https://openreview.net/forum?noteId=xyz&id=abc123XYZ#discussion")["reviews"] == 2


def test_the_reader_hands_the_page_text_to_the_server(monkeypatch):
    monkeypatch.setattr(server.papers, "get", lambda paper_id: paper(paper_id, url=FORUM))
    client = server.app.test_client()
    answer = client.post("/api/reviews/y-2026-5", json={"text": ICLR, "url": FORUM})
    assert answer.status_code == 200 and answer.get_json()["ratings"]["Rating"]["values"] == [6, 8]
    assert client.get("/api/reviews").get_json()["y-2026-5"]["reviews"] == 2
    assert client.get("/api/papers/y-2026-5").get_json()["reviews"]["reviews"] == 2
    assert client.post("/api/reviews/y-2026-5", json={"text": 5, "url": FORUM}).status_code == 400
    assert client.post("/api/reviews/y-2026-5", json={"text": ICLR}).status_code == 400
    assert client.post("/api/reviews/y-2026-6", json={"text": "nothing yet", "url": FORUM}).get_json() is None
    assert client.post("/api/reviews/y-2026-7", json={"text": "x" * (5 * 1024 * 1024), "url": FORUM}).status_code == 413
