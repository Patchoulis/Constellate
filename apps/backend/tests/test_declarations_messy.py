"""The same reading, on text as PDFs really give it, and when sources fail: every case here was once read wrongly."""

import pytest
import requests

from conftest import paper
from rp import proceedings, server, store


def item(number, title, answer, justification="Justification: Because.\n", guidelines=True):
    return f"{number}. {title}\nQuestion: Does the paper do this?\n{answer}\n{justification}" + (
        "Guidelines:\n• The answer NA means that it does not apply.\n" if guidelines else ""
    )


def checklist(*items):
    return "Some paper text.\n\nNeurIPS Paper Checklist\n" + "".join(items)


def answers(text):
    return {entry["number"]: entry["answer"] for entry in proceedings.read_declarations(text)["checklist"]}


def flags(text):
    return proceedings.read_declarations(text)["flags"]


def test_an_answer_written_another_way_is_still_that_items_answer():
    text = checklist(
        item(15, "Institutional review board (IRB) approvals", "Answer: [N/A]"),
        item(16, "Declaration of LLM usage", "Answer: [Yes]"),
    )
    assert answers(text) == {15: "NA", 16: "Yes"}
    assert flags(text) == ["llm_declared"]

    kerned = checklist(
        item(2, "Limitations", "Answer: [Y es]"), item(5, "Open access to data and code", "Answer: [No]")
    )
    assert answers(kerned) == {2: "Yes", 5: "No"}
    assert flags(kerned) == ["closed_code"]

    bare = checklist(
        item(2, "Limitations", "Answer: No"), item(5, "Open access to data and code", "Answer: [Yes] See Section 3.")
    )
    assert answers(bare) == {2: "No", 5: "Yes"}
    assert flags(bare) == ["no_limitations"]


def test_an_item_that_cannot_be_read_is_left_out_and_takes_nothing_from_the_next():
    text = checklist(item(2, "Limitations", "Answer: [TODO]"), item(5, "Open access to data and code", "Answer: [No]"))
    assert answers(text) == {5: "No"}
    assert flags(text) == ["closed_code"]

    unjustified = checklist(
        item(2, "Limitations", "Answer: [Yes]", justification=""),
        item(5, "Open access to data and code", "Answer: [No]", "Justification: Proprietary.\n"),
    )
    read = {entry["number"]: entry for entry in proceedings.read_declarations(unjustified)["checklist"]}
    assert read[2]["justification"] == "" and read[5]["justification"] == "Proprietary."


def test_ligatures_and_wrapped_lines_inside_the_checklist():
    text = checklist(
        item(2, "Limitations", "Answer: [Yes]", "Justiﬁcation: We discuss them, see Section\n3. We have several.\n"),
        item(5, "Open access to data and code", "Answer: [No]"),
    )
    read = {entry["number"]: entry for entry in proceedings.read_declarations(text)["checklist"]}
    assert set(read) == {2, 5}  # "3. We have several." is not an item
    assert read[2]["justification"] == "We discuss them, see Section 3. We have several."
    assert read[5]["answer"] == "No"


def test_flags_follow_what_an_item_is_about_not_its_number():
    moved = checklist(
        item(3, "Limitations", "Answer: [No]"),
        item(17, "Crowdsourcing and research with human subjects", "Answer: [Yes]"),
        item(
            18,
            "Institutional review board (IRB) approvals or equivalent for research with human subjects",
            "Answer: [Yes]",
        ),
        item(19, "Declaration of LLM usage", "Answer: [NA]"),
    )
    assert flags(moved) == ["no_limitations", "human_subjects"]


ICLR = """
5 CONCLUSION
We presented a method.

ETHICS STATEMENT
This work uses only public datasets and raises no privacy concerns that we are aware of at this time.

REPRODUCIBILITY STATEMENT
All code and configuration files are released with the paper, and every experiment has a fixed random seed.

THE USE OF LARGE LANGUAGE MODELS
We used a large language model to polish the wording of the manuscript and to help draft plotting code.

REFERENCES
A. Author. Models that do not need to be used twice. 2024.
"""


def test_a_statement_ends_at_the_next_heading_also_in_capitals():
    said = {s["kind"]: s["text"] for s in proceedings.read_declarations(ICLR)["statements"]}
    assert said["ethics"].endswith("at this time.")
    assert said["reproducibility"].endswith("fixed random seed.")
    assert said["llm"].endswith("plotting code.")
    assert flags(ICLR) == ["llm_declared"]  # the reference's "do not need to be used" is not theirs


def test_a_short_line_of_running_text_does_not_end_a_statement():
    text = (
        "\nImpact Statement\nThis paper presents work whose goal is to advance machine learning.\n"
        "A Large fraction of the training data comes from public sources,\nwhich we consider low risk overall.\n"
        "\nReferences\nAuthor, A. 2024.\n"
    )
    (statement,) = proceedings.read_declarations(text)["statements"]
    assert statement["text"].endswith("low risk overall.")

    appendix = "\nImpact Statement\nThis paper presents work whose goal is to advance machine learning.\nB Additional Experiments\nMore.\n"
    (statement,) = proceedings.read_declarations(appendix)["statements"]
    assert statement["text"].endswith("machine learning.")


def test_a_sentence_that_wrapped_onto_the_word_is_not_a_heading():
    text = "\nWe now turn to the method and then to its\nlimitations.\nThe next paragraph talks about something else entirely, at some length.\n"
    assert proceedings.read_declarations(text)["statements"] == []


def test_common_other_headings():
    text = "\n7 Limitations and Broader Impacts\nOur method needs a GPU with a lot of memory, which limits who can use it today.\n\nReferences\n"
    (statement,) = proceedings.read_declarations(text)["statements"]
    assert statement["kind"] == "limitations"


@pytest.mark.parametrize(
    "statement, declared",
    [
        (
            "We used an LLM only to polish the writing; it was not used for research ideation or for the experiments.",
            True,
        ),
        ("LLMs were used to check grammar in this manuscript. No other use was made of them in this work.", True),
        ("Large language models helped with editing the text of this paper for clarity and concision.", True),
        ("We did not use large language models in any part of this research or in writing the paper itself.", False),
        ("No LLMs were used in the preparation of this manuscript or in the research it describes at all.", False),
        ("This work was carried out without the assistance of any large language model at any stage of it.", False),
    ],
)
def test_whether_a_statement_declares_use(statement, declared):
    assert (flags(f"\nUse of LLMs\n{statement}\n\nReferences\n") == ["llm_declared"]) is declared


@pytest.mark.parametrize(
    "statement, declared",
    [
        (
            "We did not use LLMs for anything other than polishing the writing of this manuscript before submission.",
            True,
        ),
        ("LLMs were not used in this work, except for polishing the grammar of a few paragraphs of the paper.", True),
        ("Apart from grammar editing of the final manuscript, no LLM was used at any point of this research.", True),
        ("We made no use of language models beyond minor grammar corrections to the text of the manuscript.", True),
        ("ChatGPT helped with wording in several sections of the manuscript; all content is the authors' own.", True),
        ("This section is intentionally left empty by the authors, as required by the submission template.", False),
    ],
)
def test_exceptions_to_a_denial_are_declared_use_and_an_empty_statement_declares_nothing(statement, declared):
    assert (flags(f"\nUse of LLMs\n{statement}\n\nReferences\n") == ["llm_declared"]) is declared


def test_an_answer_on_the_line_after_its_label():
    text = checklist(
        item(2, "Limitations", "Answer:\n[No]"), item(5, "Open access to data and code", "Answer:  \n [Yes]")
    )
    assert answers(text) == {2: "No", 5: "Yes"}
    assert flags(text) == ["no_limitations"]


@pytest.mark.parametrize(
    "line, heading",
    [
        ("A Large fraction of", False),
        ("3 We then use", False),
        ("B Additional Experiments", True),
        ("6 Related work", True),
        ("A. Proofs", True),
        ("4.2 Results on ImageNet", True),
        ("REFERENCES", True),
        ("Acknowledgments", True),
        ("the limitations.", False),
        ("A Proofs of the Main Theorems", True),
    ],
)
def test_what_reads_as_a_heading(line, heading):
    assert proceedings._is_heading(line) is heading


def test_page_furniture_is_not_part_of_a_statement():
    text = (
        "\nETHICS STATEMENT\nThis work uses only public datasets and we thought about it\n12\n"
        "Published as a conference paper at ICLR 2026\ncarefully before releasing anything at all.\n\nREFERENCES\n"
    )
    (statement,) = proceedings.read_declarations(text)["statements"]
    assert (
        statement["text"]
        == "This work uses only public datasets and we thought about it carefully before releasing anything at all."
    )


def test_a_numbered_section_about_language_models_is_the_papers_subject():
    body = (
        "\n2 Use of Large Language Models\nPractitioners use large language models for many tasks, which we survey here.\n"
        + "Text.\n" * 400
    )
    assert flags(body) == []
    late = (
        "Text.\n" * 400
        + "\n9 Use of Large Language Models\nWe used an LLM to polish the wording of the manuscript before submission.\n"
    )
    assert flags(late) == ["llm_declared"]


# ------------------------------------------------------------------ sources that are incomplete or fail
def test_a_paper_without_a_pdf_is_not_given_its_neighbours(monkeypatch):
    pages = {
        "https://proceedings.mlr.press/": '<li><a href="v306"><b>Volume 306</b></a> Proceedings of ICML 2026</li>',
        "https://proceedings.mlr.press/v306/": '<div class="paper"><p class="title">First Paper</p><p class="links">[<a href="a.html">abs</a>]</p></div>'
        '<div class="paper"><p class="title">Don&#39;t Stop: Naïve Baselines</p><p class="links">[<a href="b26.pdf">Download PDF</a>]</p></div>'
        '<div class="paper"><p class="title">Same Title</p><p class="links">[<a href="c.pdf">PDF</a>]</p></div>'
        '<div class="paper"><p class="title">Same Title</p><p class="links">[<a href="d.pdf">PDF</a>]</p></div>',
    }
    monkeypatch.setattr(proceedings, "_get", lambda url: pages[url])
    index = proceedings._icml("2026")
    assert proceedings._key("First Paper") not in index
    assert index[proceedings._key("Don't stop: Naive baselines")] == "https://proceedings.mlr.press/v306/b26.pdf"
    assert proceedings._key("Same Title") not in index  # two papers, one title: no guess


def test_a_list_that_cannot_be_fetched_is_asked_for_once_and_the_paper_is_just_not_readable(monkeypatch):
    asked = []

    def fail(url):
        asked.append(url)
        raise requests.ConnectionError("no network")

    monkeypatch.setattr(proceedings, "_get", fail)
    monkeypatch.setattr(proceedings, "_indexes", {})
    for _ in range(3):
        assert proceedings.pdf_url(paper("wacv-2031-1", title="Anything")) is None
    assert len(asked) == 1


class _Response:
    def __init__(self, body=b"", status=200, location=None):
        self.body, self.status_code, self.headers = body, status, {"Location": location} if location else {}

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def close(self):
        pass

    def iter_content(self, size):
        for start in range(0, len(self.body), size):
            yield self.body[start : start + size]


PDF = b"%PDF-1.7 ..."


@pytest.mark.parametrize(
    "pages, fine",
    [
        ({"https://arxiv.org/pdf/1": _Response(PDF)}, True),
        (
            {
                "https://arxiv.org/pdf/1": _Response(status=302, location="/pdf/1v2"),
                "https://arxiv.org/pdf/1v2": _Response(PDF),
            },
            True,
        ),
        # every hop of a redirect must be a publisher's host, over https: checked before it is fetched
        (
            {
                "https://arxiv.org/pdf/1": _Response(status=302, location="https://evil.example/x.pdf"),
                "https://evil.example/x.pdf": _Response(PDF),
            },
            False,
        ),
        (
            {
                "https://arxiv.org/pdf/1": _Response(status=301, location="http://127.0.0.1:8765/api/runs"),
                "http://127.0.0.1:8765/api/runs": _Response(PDF),
            },
            False,
        ),
        (
            {
                "https://arxiv.org/pdf/1": _Response(status=302, location="http://arxiv.org/pdf/1"),
                "http://arxiv.org/pdf/1": _Response(PDF),
            },
            False,
        ),
        ({"https://arxiv.org/pdf/1": _Response(b"<html>not a pdf</html>")}, False),
        ({"https://arxiv.org/pdf/1": _Response(PDF, status=404)}, False),
        (
            {"https://arxiv.org/pdf/1": _Response(b"%PDF" + b"0" * 4096)},
            False,
        ),  # larger than allowed (limit lowered below)
        ({"https://arxiv.org/pdf/1": _Response(status=302, location="https://arxiv.org/pdf/1")}, False),  # a loop
    ],
)
def test_only_a_pdf_of_sane_size_from_a_publisher_is_accepted(monkeypatch, pages, fine):
    fetched = []

    def get(address, **options):
        assert options["allow_redirects"] is False
        fetched.append(address)
        return pages[address]

    monkeypatch.setattr(proceedings.requests, "get", get)
    monkeypatch.setattr(proceedings, "MAX_PDF", 1024)
    if fine:
        assert proceedings._download("https://arxiv.org/pdf/1") == PDF
    else:
        with pytest.raises(LookupError):
            proceedings._download("https://arxiv.org/pdf/1")
    assert not any("evil" in address or "127.0.0.1" in address or address.startswith("http://") for address in fetched)


def test_an_address_elsewhere_is_never_fetched(monkeypatch):
    monkeypatch.setattr(proceedings.requests, "get", lambda *a, **k: pytest.fail("fetched"))
    for address in ("https://evil.example/a.pdf", "http://arxiv.org/pdf/1", "file:///C:/secret.pdf"):
        with pytest.raises(LookupError):
            proceedings._download(address)


def test_reading_many_papers_counts_what_happened_and_saves_in_batches(monkeypatch):
    store.write_json(proceedings.DECLARATIONS, {})
    saves = []

    def read(one, folder):
        if one["id"].endswith("3"):
            raise LookupError(one["id"])
        if one["id"].endswith("4"):
            raise ValueError("no text")
        return {"parser": proceedings.PARSER, "checklist": [], "statements": [], "flags": []}

    monkeypatch.setattr(proceedings, "_read_paper", read)
    keep = proceedings._keep
    monkeypatch.setattr(proceedings, "_keep", lambda batch: (saves.append(len(batch)), keep(batch)))
    lines = []
    assert proceedings.read_all([paper(f"x-2026-{n}") for n in range(1, 7)], lines.append, batch=4) == 4
    assert saves == [2, 2]  # not one rewrite of the file per paper
    assert lines[-1] == "  4 papers read; 1 have no PDF a program may fetch; 1 could not be read"
    assert proceedings.read_all([paper(f"x-2026-{n}") for n in (1, 2)], lines.append) == 0  # nothing is read twice


def test_a_pdf_without_text_is_not_recorded_as_declaring_nothing(monkeypatch, tmp_path):
    import pypdf

    class Empty:
        pages = []

    monkeypatch.setattr(pypdf, "PdfReader", lambda path: Empty())
    monkeypatch.setattr(proceedings, "fetch_pdf", lambda one, folder: tmp_path / "x.pdf")
    with pytest.raises(ValueError, match="no text"):
        proceedings.declarations(paper("x-2026-99"), tmp_path)
    assert proceedings.known("x-2026-99") is None


# ------------------------------------------------------------------ who may ask the server to do things
def test_only_the_apps_own_interface_may_change_things(monkeypatch):
    client = server.app.test_client()

    def post(origin):
        return client.post("/api/runs/nothing/declarations", headers={"Origin": origin} if origin else {}).status_code

    monkeypatch.delenv("RP_ORIGIN", raising=False)
    assert post("http://127.0.0.1:5555") == 404  # development: any page from this machine
    assert post("https://example.org") == 403
    monkeypatch.setenv("RP_ORIGIN", "http://127.0.0.1:4000")
    assert post("http://127.0.0.1:4000") == 404  # allowed through: the run just does not exist
    assert post("http://127.0.0.1:5555") == 403  # another program's page on this machine
    assert post("http://localhost:4000") == 403
    assert post(None) == 404  # not a browser: nothing to forge


def test_opening_a_paper_works_when_its_proceedings_cannot_be_reached(monkeypatch):
    def fail(url):
        raise requests.ConnectionError("no network")

    monkeypatch.setattr(proceedings, "_get", fail)
    monkeypatch.setattr(proceedings, "_indexes", {})
    monkeypatch.setattr(server.papers, "get", lambda paper_id: paper(paper_id, title="Anything"))
    answer = server.app.test_client().get("/api/papers/cvpr-2031-7")
    assert answer.status_code == 200 and answer.get_json()["readable"] is False


@pytest.mark.parametrize(
    "statement, declared",
    [
        ("LLMs were not involved in any part of this research, which is limited to classical methods.", False),
        ("We do not use LLMs. Our analysis is limited to the datasets described in Section 4 of the paper.", False),
        ("No language models were used. The code is released only for research purposes, under a licence.", False),
        ("We used GPT-4 to polish the writing of this manuscript, not to generate any of its ideas.", True),
        ("We used ChatGPT to polish writing and did not use it for research ideation or the experiments.", True),
        ("LLMs were not used for writing, editing and proofreading of any part of this paper at all.", False),
        ("No LLM was used at any point of this work, not even for editing the text of the manuscript.", False),
        ("We did not use LLMs for ideation, but used them for polishing the final text of the paper.", True),
    ],
)
def test_a_denial_in_one_half_of_a_sentence_does_not_cancel_a_use_in_the_other(statement, declared):
    assert (flags(f"\nUse of LLMs\n{statement}\n\nReferences\n") == ["llm_declared"]) is declared
