"""Acceptance states, what authors declare, and the filters built on them: across conferences and formats."""
import numpy as np
import pytest

from conftest import paper
from rp import pipeline, proceedings, settings, store
from rp.library import acceptance
from rp.selection import Selection

NEURIPS = """
5 Conclusion
We presented a method.

Limitations and Future Work
Our analysis assumes bounded gradients, which may not hold for very deep networks trained at scale.

References
[1] A. Author. A paper. 2024.

NeurIPS Paper Checklist
1. Claims
Question: Do the main claims made in the abstract and introduction accurately reflect the paper's contributions?
Answer: [Yes]
Justification: The claims match the results.
Guidelines:
• The answer NA means that the abstract and introduction do not include the claims.
2. Limitations
Question: Does the paper discuss the limitations of the work performed by the authors?
Answer: [No]
Justification: We did not have space.
Guidelines:
• The answer NA means that the paper has no limitation.
5. Open access to data and code
Question: Does the paper provide open access to the data and code?
Answer: [No]
Justification: The code is proprietary.
Guidelines:
• The answer NA means that paper does not include experiments requiring code.
14. Crowdsourcing and research with human subjects
Question: For crowdsourcing experiments and research with human subjects, does the paper include instructions?
Answer: [NA]
Justification: The paper does not involve crowdsourcing nor research with human subjects.
Guidelines:
• The answer NA means that the paper does not involve crowdsourcing.
15. Institutional review board (IRB) approvals or equivalent for research with human
subjects
Question: Does the paper describe potential risks incurred by study participants?
Answer: [NA]
Justification: Not applicable.
Guidelines:
• The answer NA means that the paper does not involve crowdsourcing.
16. Declaration of LLM usage
20
Question: Does the paper describe the usage of LLMs if it is an important component of the core methods?
Answer: [Yes]
Justification: An LLM generates the candidate programs that our method searches over.
Guidelines:
• The answer NA means that the core method development does not involve LLMs.
"""

ICML = """
6. Conclusion
We presented a method.

Impact Statement
This paper presents work whose goal is to advance the field of Machine Learning. There are many potential
societal consequences of our work, none which we feel must be specifically highlighted here.

References
Author, A. A paper. 2024.
"""

ICLR = """
ETHICS STATEMENT
This work uses only public datasets and raises no privacy concerns that we are aware of at this time.

REPRODUCIBILITY STATEMENT
All code and configuration files are released with the paper, and every experiment has a fixed random seed.

THE USE OF LARGE LANGUAGE MODELS
We used a large language model to polish the wording of the manuscript and to help draft plotting code.

REFERENCES
"""

DENIES = """
A. Use of LLMs
We did not use large language models in any part of this research or in writing the paper itself.

B Additional Experiments
More results.
"""


def test_neurips_checklist_is_read_with_multi_line_titles_and_page_numbers():
    declared = proceedings.read_declarations(NEURIPS)
    answers = {item["number"]: item["answer"] for item in declared["checklist"]}
    assert answers == {1: "Yes", 2: "No", 5: "No", 14: "NA", 15: "NA", 16: "Yes"}
    by_number = {item["number"]: item for item in declared["checklist"]}
    assert by_number[15]["title"] == "Institutional review board (IRB) approvals or equivalent for research with human subjects"
    assert by_number[16]["title"] == "Declaration of LLM usage"                    # the page number "20" is not part of it
    assert by_number[16]["justification"].startswith("An LLM generates the candidate programs")
    assert declared["flags"] == ["llm_declared", "closed_code", "no_limitations"]


def test_statements_are_found_outside_the_checklist_only():
    declared = proceedings.read_declarations(NEURIPS)
    (statement,) = declared["statements"]
    assert statement["kind"] == "limitations" and statement["text"].startswith("Our analysis assumes bounded gradients")
    assert "References" not in statement["text"] and "Checklist" not in statement["text"]


def test_icml_impact_statement():
    declared = proceedings.read_declarations(ICML)
    assert declared["checklist"] == [] and declared["flags"] == []
    (statement,) = declared["statements"]
    assert statement["kind"] == "impact" and statement["text"].startswith("This paper presents work whose goal")
    assert "Author, A." not in statement["text"]


def test_iclr_style_statements_in_capitals_and_a_stated_llm_use():
    declared = proceedings.read_declarations(ICLR)
    assert {s["kind"] for s in declared["statements"]} == {"ethics", "reproducibility", "llm"}
    assert declared["flags"] == ["llm_declared"]


def test_a_statement_that_says_no_llm_was_used_raises_no_flag():
    declared = proceedings.read_declarations(DENIES)
    (statement,) = declared["statements"]
    assert statement["kind"] == "llm" and "Additional Experiments" not in statement["text"]
    assert declared["flags"] == []


def test_a_section_that_merely_starts_with_the_word_is_not_a_statement():
    text = "\n4.2 Limitations of Linear Pooling Representations\nExisting methods pool tokens linearly, which loses order.\n"
    assert proceedings.read_declarations(text) == {"checklist": [], "statements": [], "flags": []}
    assert proceedings.read_declarations("Just a paper with nothing declared.") == {"checklist": [], "statements": [], "flags": []}


@pytest.mark.parametrize("decision, tier", [
    ("Accept (oral)", "oral"), ("Accept (Oral)", "oral"), ("Award Candidate", "oral"),
    ("Accept (spotlight)", "spotlight"), ("Highlight", "spotlight"),
    ("Accept (poster)", "poster"), ("Accept (regular)", "poster"), ("Accept", "poster"), ("", "poster"),
])
def test_acceptance_is_read_the_same_whatever_the_conference_calls_it(decision, tier):
    assert acceptance(paper("cvpr-2026-123", comment=decision)) == tier


def test_an_arxiv_paper_has_no_acceptance_state():
    assert acceptance(paper("2512.01234", comment="Accepted as an oral at NeurIPS 2025")) is None


def test_pdf_addresses_come_from_where_each_conference_publishes(monkeypatch):
    pages = {
        "https://proceedings.neurips.cc/paper_files/paper/2025": '<a href="/paper_files/paper/2025/vol38-main">main</a>',
        "https://proceedings.neurips.cc/paper_files/paper/2025/vol38-main":
            '<a href="/paper_files/paper/2025/hash/abc123-Abstract-Conference.html">A <i>Great</i> Paper</a>',
        "https://proceedings.mlr.press/": '<li><a href="v306"><b>Volume 306</b></a> Proceedings of ICML 2026</li>',
        "https://proceedings.mlr.press/v306/":
            '<div class="paper"><p class="title">Another Paper</p><p class="links">[<a href="x.html">abs</a>]'
            '[<a href="https://raw.githubusercontent.com/mlresearch/v306/main/assets/a26/a26.pdf">Download PDF</a>]</p></div>',
        "https://openaccess.thecvf.com/CVPR2026?day=all":
            '<dt class="ptitle"><br><a href="/content/CVPR2026/html/Doe_Vision_CVPR_2026_paper.html">Vision Paper</a></dt>',
    }
    monkeypatch.setattr(proceedings, "_get", lambda url: pages[url])
    assert proceedings.pdf_url(paper("neurips-2025-1", title="A Great Paper")) == \
        "https://proceedings.neurips.cc/paper_files/paper/2025/file/abc123-Paper-Conference.pdf"
    assert proceedings.pdf_url(paper("icml-2026-2", title="Another  paper")).endswith("/a26/a26.pdf")
    assert proceedings.pdf_url(paper("cvpr-2026-3", title="Vision Paper")) == \
        "https://openaccess.thecvf.com/content/CVPR2026/papers/Doe_Vision_CVPR_2026_paper.pdf"
    assert proceedings.pdf_url(paper("neurips-2025-9", title="Not In The Proceedings")) is None
    assert proceedings.pdf_url(paper("iclr-2026-4", title="A Great Paper")) is None          # nowhere to fetch from
    assert proceedings.pdf_url(paper("2512.01234", pdf_url="https://arxiv.org/pdf/2512.01234")) == "https://arxiv.org/pdf/2512.01234"


def _run(papers, flagged=None, **changed):
    run = pipeline.Run(Selection.parse(venues=["X 2026"]), settings.load() | {"min_community_size": 1} | changed,
                       log=lambda line: None)
    run.papers = papers
    run.embeddings = np.arange(len(papers), dtype=np.float32).reshape(-1, 1)
    run.flagged = np.array(flagged if flagged is not None else [False] * len(papers))
    return run


def test_filters_leave_out_what_the_settings_say_and_keep_rows_aligned():
    papers = [paper("x-2026-1", comment="Accept (oral)"), paper("x-2026-2", comment="Accept (poster)"),
              paper("x-2026-3", comment="Accept (spotlight)"), paper("x-2026-4", comment="Accept (poster)"),
              paper("2601.00001"), paper("2601.00002"), paper("2601.00003")]
    now = {"parser": proceedings.PARSER}
    store.write_json(proceedings.DECLARATIONS, {"x-2026-3": now | {"flags": ["llm_declared"]}, "x-2026-1": now | {"flags": []},
                                                "2601.00002": {"parser": 0, "flags": ["llm_declared"]}})   # an older reading: not trusted

    everything = _run(papers)
    pipeline.filter_papers(everything)
    assert len(everything.papers) == 7

    highlights = _run(papers, acceptance_filter="highlights")
    pipeline.filter_papers(highlights)
    assert [p["id"] for p in highlights.papers] == ["x-2026-1", "x-2026-3", "2601.00001", "2601.00002", "2601.00003"]   # arXiv papers are not judged
    assert highlights.embeddings.ravel().tolist() == [0, 2, 4, 5, 6]

    strict = _run(papers, flagged=[False, True, False, False, False, False, False], acceptance_filter="highlights",
                  include_ai_papers=False, exclude_flags=["llm_declared"])
    pipeline.filter_papers(strict)
    assert [p["id"] for p in strict.papers] == ["x-2026-1", "2601.00001", "2601.00002", "2601.00003"]
    assert len(strict.embeddings) == len(strict.flagged) == 4


def test_filtering_down_to_too_few_papers_is_refused_with_a_reason():
    run = _run([paper("x-2026-1", comment="Accept (poster)")] * 3, acceptance_filter="oral", min_community_size=2)
    with pytest.raises(ValueError, match="at least 8"):
        pipeline.filter_papers(run)


def test_filter_settings_are_checked():
    assert settings.save({"exclude_flags": ["closed_code", "llm_declared"]})["exclude_flags"] == ["llm_declared", "closed_code"]
    for bad in ({"exclude_flags": ["nonsense"]}, {"exclude_flags": "llm_declared"}, {"acceptance_filter": "best"}):
        with pytest.raises(ValueError):
            settings.save(bad)
    settings.save({"exclude_flags": [], "acceptance_filter": "all"})
