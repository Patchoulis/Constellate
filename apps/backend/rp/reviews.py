"""The scores reviewers gave a paper, read from the text of its reviews page.

Reviews are on OpenReview, which does not serve them to programs, so nothing here fetches anything.  When the
user opens a paper's Reviews tab, the interface hands over the text of the page it is showing; the scores are read
from that and kept, and from then on they are shown with the paper and in the graph.

The page is a thread of notes, each under a heading: "Official Review of Submission123 by Reviewer aBcD",
"Official Comment by Authors", "Meta Review of ...", "Decision by ...".  Only the reviews are read, each for one
score per field, so a rebuttal quoting a score, a comment revising one, or a meta-review's confidence is not
counted as a review.  A review lists its scores as "Field: number" or "Field: number: what it means":

    Rating: 5: Accept              Quality: 2: fair
    Confidence: 4                  Soundness: 3 good

Which fields exist and what their scales are differs by conference and year (NeurIPS 2025: quality, clarity,
significance and originality from 1 to 4, a rating from 1 to 6; ICLR: soundness, presentation and contribution
from 1 to 4, a rating from 1 to 10), so the scores are kept as given, by field, and never mixed into one number.
A page with scores but no review headings (another site's layout) is read as a whole, and says so ("exact": false).
"""
import re

from . import LIBRARY, store

REVIEWS = LIBRARY / "reviews.json"                 # {paper id: {"reviews", "exact", "ratings": {field: {...}}}}
MAX_TEXT = 2_000_000                               # characters of page text accepted
MAX_REVIEWS = 40                                   # more than this is not one paper's reviews

# the fields, in the order shown; the overall judgement first
FIELDS = ["Rating", "Overall Recommendation", "Overall", "Recommendation", "Quality", "Clarity", "Significance",
          "Originality", "Soundness", "Presentation", "Contribution", "Confidence"]
_FIELD = "(?:" + "|".join(FIELDS) + ")"
_GAP = r"[^\S\n]*(?:\n[^\S\n]*){0,2}"              # spaces of any kind (web text has no-break ones), up to two line ends
# "Field: 3", "Field: 3: good", "Field: 3 good"; not "3.5", "3/5", "3 out of 4", "3 of the proofs", "5 -> 7"
_SCORE = re.compile(
    r"(?mi)^[^\S\n]*(" + _FIELD + r")[^\S\n]*:" + _GAP + r"(\d{1,2})(?![\d.,/]|\s*(?:/|->|out of|of\b))"
    r"[^\S\n]*(?:([:\-–])[^\S\n]*)?([^\n\d]*)")
_INLINE = re.compile(r"(?i)(?<=\S)[^\S\n]+(?=" + _FIELD + r"[^\S\n]*:[^\S\n]*\d)")
_NOTE = re.compile(r"(?mi)^[^\S\n]*(Official Review|Official Comment|Public Comment|Meta[- ]?Review|Author Rebuttal|"
                   r"Rebuttal|Paper Decision|Decision|Author[^\n]{0,30}Comment|Ethics Review|Withdrawal)\b[^\n]{0,160}$")
_NAMES = {field.lower(): field for field in FIELDS}


def _scores(text: str, once: bool) -> list:
    """[(field, score, meaning)] in `text`; with `once`, only the first of each field (one review gives one)."""
    lines = _INLINE.sub("\n", text)                # fields that share a line each get their own
    found, seen = [], set()
    for field, score, separator, meaning in _SCORE.findall(lines):
        field, meaning = _NAMES[field.lower()], " ".join(meaning.split()).strip(" .:")
        if (once and field in seen) or (not separator and len(meaning.split()) > 1):   # prose, not a score's meaning
            continue
        seen.add(field)
        found.append((field, int(score), meaning[:60]))
    return found


def read_ratings(text: str) -> dict | None:
    """{"reviews": how many reviews gave scores, "exact": were they told apart by their headings,
    "ratings": {field: {"values", "mean", "meanings"}}}, or None if the text has no scores."""
    text = text[:MAX_TEXT]
    notes = list(_NOTE.finditer(text))
    blocks = [text[note.end():following.start() if following else len(text)]
              for note, following in zip(notes, notes[1:] + [None]) if note.group(1).lower() == "official review"]
    per_review = [scores for scores in (_scores(block, once=True) for block in blocks) if scores]
    exact = bool(per_review)
    if not exact:                                  # no headings to go by: every score on the page
        per_review = [_scores(text, once=False)]
    ratings = {}
    for scores in per_review[:MAX_REVIEWS]:
        for field, score, meaning in scores:
            entry = ratings.setdefault(field, {"values": [], "meanings": []})
            if len(entry["values"]) < MAX_REVIEWS:
                entry["values"].append(score)
                entry["meanings"].append(meaning)
    if not ratings:
        return None
    for entry in ratings.values():
        entry["mean"] = round(sum(entry["values"]) / len(entry["values"]), 2)
    ordered = {field: ratings[field] for field in FIELDS if field in ratings}
    counts = sorted(len(entry["values"]) for entry in ordered.values())
    # without headings, the usual number of scores per field is the best guess at how many reviews there are
    return {"reviews": min(len(per_review), MAX_REVIEWS) if exact else counts[len(counts) // 2], "exact": exact,
            "ratings": ordered}


_read = {"stamp": None, "content": {}}


def stored() -> dict:
    """{paper id: its ratings}, for every paper whose reviews page has been read."""
    stamp = REVIEWS.stat().st_mtime_ns if REVIEWS.exists() else None
    if stamp != _read["stamp"]:
        _read.update(stamp=stamp, content=store.read_json(REVIEWS) or {})
    return _read["content"]


def known(paper_id: str) -> dict | None:
    return stored().get(paper_id)


def forum(address: str | None) -> str | None:
    """The id of the OpenReview discussion an address points at."""
    found = re.search(r"openreview\.net/[^?#]*\?(?:[^#]*&)?id=([\w-]+)", address or "")
    return found.group(1) if found else None


def keep(paper: dict, text: str, shown: str | None) -> dict | None:
    """Read the scores in a reviews page's text and keep them.  `shown` is the address of the page the text came
    from: if the reader has wandered to another paper's page, nothing is read.  A page without scores (not loaded
    yet, or the reviews are not public), or with fewer reviews than were read before (still loading), changes
    nothing: what was read before stays."""
    before = known(paper["id"])
    if forum(paper.get("url")) is None or forum(shown) != forum(paper.get("url")):
        return before
    ratings = read_ratings(text)
    if ratings is None or (before and (before.get("exact", False), before["reviews"]) > (ratings["exact"], ratings["reviews"])):
        return before
    store.update_json(REVIEWS, lambda content: content.__setitem__(paper["id"], ratings))
    return ratings
