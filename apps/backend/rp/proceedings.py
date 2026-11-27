"""What a paper's published PDF adds: the PDF itself, and what its authors declare in it.

Where the PDF comes from depends on who published the paper:

    NeurIPS             proceedings.neurips.cc
    ICML                proceedings.mlr.press (PMLR; one volume per year)
    CVPR, ICCV, WACV    openaccess.thecvf.com
    arXiv               arxiv.org
    ICLR and others     nowhere a program may fetch from (OpenReview does not serve them to programs)

What authors declare comes in two forms, and a paper is read for both, whatever its venue:

  a checklist    NeurIPS papers end with the "NeurIPS Paper Checklist": numbered questions answered [Yes] / [No] /
                 [NA] with a justification.  Among them: are limitations discussed, are code and data open, were
                 human subjects involved, and were language models an important part of the method ("Declaration
                 of LLM usage").
  statements     Other venues ask for free-text sections instead: an "Impact Statement" (ICML), an "Ethics
                 Statement" or "Reproducibility Statement" (ICLR), a section on the use of language models (ICLR
                 2026 and others), or authors add them unasked.  These are found by their headings.

From both come a few flags (FLAGS), which are what the interface shows and what a run can be filtered by.  A flag
is the authors' own statement, so it is as reliable as they are, and absence of a flag is not a statement: NeurIPS,
for one, asks for no declaration when a model was only used for writing or editing, while a free-text statement
elsewhere often declares exactly that.  The same flag therefore means somewhat different things at different venues.

The text comes out of a PDF with broken lines, page numbers, ligatures and stray spaces, so the reading is
tolerant and, where it cannot be sure, says nothing rather than guess: a checklist answer that is not clearly
Yes / No / NA is left out, and a paper with no readable text is not recorded as having declared nothing.

What is NOT here: reviews and the issues reviewers raised.  They are on OpenReview, which does not serve them to
programs; a paper's OpenReview page (the Reviews tab of the reader) shows them.

Everything fetched is kept: each conference's list of PDFs, and what was read from each paper (with the version
of the reader that read it, so a better reader reads again).
"""

import html
import re
import tempfile
import time
import unicodedata
from pathlib import Path
from urllib.parse import urljoin, urlparse

import requests

from . import LIBRARY, store

HEADERS = {"User-Agent": "research-partitioner/0.1 (python-requests)"}
FOLDER = LIBRARY / "proceedings"
DECLARATIONS = FOLDER / "declarations.json"  # {paper id: {"parser", "checklist", "statements", "flags"}}
PARSER = 2  # raise when the reading below changes: older readings are redone
PDF_HOSTS = {
    "arxiv.org",
    "proceedings.neurips.cc",
    "proceedings.mlr.press",
    "raw.githubusercontent.com",
    "openaccess.thecvf.com",
}
MAX_PDF = 80 * 1024 * 1024  # bytes
PDF_SECONDS = 180  # for the whole download
INDEX_DAYS = 30  # a conference's list of PDFs is fetched again after this long
RETRY_SECONDS = 3600  # ...and a list that could not be fetched, after this long
MIN_TEXT = 2000  # characters: less than this is a scan or a broken file, not a paper

# id -> what it says about a paper, as shown
FLAGS = {
    "llm_declared": "Authors declare the use of language models",
    "closed_code": "Code and data not openly available",
    "no_limitations": "No limitations discussed",
    "human_subjects": "Involves human subjects or crowdsourcing",
}
# id -> what raises it, for whoever wonders what the tag on a paper means
FLAG_NOTES = {
    "llm_declared": "The paper's checklist answers Yes to language models being an important part of the method, or "
    "the paper has a statement saying they were used (which, outside NeurIPS, often means for writing).",
    "closed_code": "The paper's checklist answers No to providing open access to its data and code.",
    "no_limitations": "The paper's checklist answers No to discussing the limitations of the work.",
    "human_subjects": "The paper's checklist answers Yes to crowdsourcing or research with human subjects.",
}
# What a checklist item is about (found in its title, since its number changes between years and tracks)
# -> (flag, the answer that raises it)
CHECKLIST_FLAGS = [
    (r"\bLLM|language model", "llm_declared", "Yes"),
    (r"open access", "closed_code", "No"),
    (r"^limitations", "no_limitations", "No"),
    (r"^crowdsourcing", "human_subjects", "Yes"),
]


def _key(title: str) -> str:
    """A title as it is compared between a conference's list and its proceedings: letters and digits only."""
    plain = unicodedata.normalize("NFKD", html.unescape(re.sub(r"<[^>]+>", " ", title)))
    return re.sub(r"[^a-z0-9]+", "", plain.encode("ascii", "ignore").decode().lower())


def _by_title(pairs) -> dict:
    """{title key: address}, without the titles that two papers share: a guess there would show the wrong paper."""
    found, twice = {}, set()
    for title, address in pairs:
        key = _key(title)
        if key in found and found[key] != address:
            twice.add(key)
        found[key] = address
    return {key: address for key, address in found.items() if key and key not in twice}


def _get(url: str) -> str:
    response = requests.get(url, headers=HEADERS, timeout=60)
    response.raise_for_status()
    return response.text


# ------------------------------------------------------------------ where each conference keeps its PDFs
def _neurips(year: str) -> dict:
    host = "https://proceedings.neurips.cc"
    listing = _get(f"{host}/paper_files/paper/{year}")
    # newer years split the list by track on sub-pages; older ones list every paper on the year's page
    pages = [listing] + [
        _get(host + link) for link in sorted(set(re.findall(rf'href="(/paper_files/paper/{year}/vol[^"]+)"', listing)))
    ]
    return _by_title(
        (title, f"{host}/paper_files/paper/{year}/file/{digest}-Paper-{track}.pdf")
        for page in pages
        for digest, track, title in re.findall(
            rf'href="/paper_files/paper/{year}/hash/([0-9a-f]+)-Abstract-([\w-]+)\.html">(.*?)</a>', page, re.S
        )
    )


def _icml(year: str) -> dict:
    host = "https://proceedings.mlr.press"
    volume = re.search(rf'href="(v\d+)"><b>Volume \d+</b></a>\s*Proceedings of ICML {year}\b', _get(host + "/"))
    if not volume:
        return {}
    address = f"{host}/{volume.group(1)}/"

    def papers():
        for block in _get(address).split('<div class="paper">')[1:]:  # one block per paper: a paper without
            title = re.search(r'<p class="title">(.*?)</p>', block, re.S)  # a PDF must not be given its neighbour's
            pdf = re.search(r'<a href="([^"]+\.pdf)"', block)
            if title and pdf:
                yield title.group(1), urljoin(address, pdf.group(1))

    return _by_title(papers())


def _cvf(name: str):
    def index(year: str) -> dict:
        host = "https://openaccess.thecvf.com"
        return _by_title(
            (title, host + path.replace("/html/", "/papers/").replace(".html", ".pdf"))
            for path, title in re.findall(
                r'<dt class="ptitle">\s*(?:<br>)?\s*<a href="(/content/[^"]+\.html)">(.*?)</a>',
                _get(f"{host}/{name}{year}?day=all"),
                re.S,
            )
        )

    return index


SOURCES = {"neurips": _neurips, "icml": _icml, "cvpr": _cvf("CVPR"), "iccv": _cvf("ICCV"), "wacv": _cvf("WACV")}
_indexes = {}  # (slug, year) -> (when it was looked up, {title key: address})


def _index(slug: str, year: str) -> dict:
    """{title key: PDF address} for one conference and year.  Kept on disk for INDEX_DAYS (proceedings appear
    track by track), and in memory, so a list that cannot be fetched is not asked for again with every paper."""
    looked, index = _indexes.get((slug, year), (0, None))
    if index is not None and time.time() - looked < (86400 if index else RETRY_SECONDS):
        return index
    path = FOLDER / f"{slug}-{year}.json"
    kept = store.read_json(path)
    if kept is None or time.time() - path.stat().st_mtime > INDEX_DAYS * 86400:
        try:
            index = SOURCES[slug](year)
        except requests.RequestException:  # not published yet, or no connection: what was kept, if anything
            index = {}
        if index:
            store.write_json(path, index)
        else:
            index = kept or {}
    else:
        index = kept
    _indexes[(slug, year)] = (time.time(), index)
    return index


def pdf_url(paper: dict) -> str | None:
    """Where this paper's PDF can be fetched from, if anywhere."""
    address = paper.get("pdf_url") or ""
    if address.startswith("https://arxiv.org/"):
        return address
    found = re.fullmatch(r"([a-z]+)-(\d{4})-\d+", paper["id"])
    if found and found.group(1) in SOURCES:
        return _index(*found.groups()).get(_key(paper["title"]))
    return None


def _file_name(paper: dict) -> str:
    return re.sub(r"[^\w.-]", "_", paper["id"]) + ".pdf"


def _download(address: str) -> bytes:
    """A PDF's bytes.  LookupError unless it is a PDF of a sane size from one of the hosts papers are published on."""
    deadline = time.time() + PDF_SECONDS
    for _ in range(6):  # redirects are followed by hand: every hop must be a publisher's host
        parts = urlparse(address)
        if parts.scheme != "https" or (parts.hostname or "").removeprefix("www.") not in PDF_HOSTS:
            raise LookupError(f"{address} is not on a host papers are published on")
        response = requests.get(address, headers=HEADERS, timeout=(10, 20), stream=True, allow_redirects=False)
        if response.status_code not in (301, 302, 303, 307, 308):
            break
        response.close()
        address = urljoin(address, response.headers.get("Location", ""))
    with response:
        if response.status_code != 200:
            raise LookupError(f"{address} did not return a PDF (HTTP {response.status_code})")
        content = bytearray()
        for chunk in response.iter_content(1 << 16):
            content += chunk
            if len(content) > MAX_PDF or time.time() > deadline:
                raise LookupError(f"{address} is too large or too slow to fetch")
    if not content.startswith(b"%PDF"):
        raise LookupError(f"{address} did not return a PDF")
    return bytes(content)


def fetch_pdf(paper: dict, folder: Path) -> Path:
    """The paper's PDF as a file: the one in `folder` or already in the library, else downloaded into `folder`.
    LookupError if it has none."""
    name = _file_name(paper)
    for path in (folder / name, LIBRARY / "pdfs" / name):
        if path.exists():
            return path
    address = pdf_url(paper)
    if address is None:
        raise LookupError(paper["id"])
    store.write_bytes(folder / name, _download(address))
    return folder / name


# ------------------------------------------------------------------ reading what authors declare
_ITEM_START = re.compile(r"(?m)^[ \t]*(\d{1,2})\.[ \t]+(?=[A-Z])")
_ANSWER = re.compile(r"Answer:\s*(?:\[([^\]\n]{1,10})\]|(Yes|No|N\s?/?\s?A)\b)", re.I)
_ANSWERS = {"yes": "Yes", "no": "No", "na": "NA"}

# Headings under which authors make a statement, whatever the venue calls it: (what it is, how the heading reads).
_HEADINGS = [
    (
        "llm",
        r"(?:Declaration (?:of|on) |Statement (?:of|on) (?:the )?|The |Disclosure of )?(?:Use|Usage) of "
        r"(?:Large Language Models|LLMs?|Generative AI|AI(?: Tools| Assistants?)?)(?: \(LLMs?\))?(?: Statement)?"
        r"|LLM (?:Usage|Use)(?: Statement| Declaration| Disclosure)?"
        r"|AI (?:Usage|Assistance|Use) (?:Statement|Disclosure|Declaration)",
    ),
    ("impact", r"(?:Broader )?Impact Statements?|Broader Impacts?|Societal Impacts?"),
    ("ethics", r"Ethics Statement|Ethical (?:Considerations|Statement)"),
    ("reproducibility", r"Reproducibility Statement"),
    ("limitations", r"Limitations(?: and (?:Future Work|Broader Impacts?|Societal Impacts?|Discussion))?"),
]
_HEADING = re.compile(
    r"(?m)^[ \t]*(?P<prefix>(?:[A-Z]|\d{1,2})?(?:\.\d{1,2})*\.?)[ \t]*(?P<title>"
    + "|".join(f"(?P<{kind}>{pattern})" for kind, pattern in _HEADINGS)
    + r")[ \t]*(?:$|[.:][ \t]*(?P<same_line>[^\n]*)$)",
    re.I,
)
_CLOSING = re.compile(
    r"(?:References|Bibliography|Acknowledge?ments?|Appendix\b.{0,40}|Appendices|"
    r"(?:NeurIPS )?Paper Checklist|Supplementary Material)\.?",
    re.I,
)
_SECTION = re.compile(r"(?P<prefix>[A-Z]|\d{1,2})(?P<rest>(?:\.\d{1,2})*)(?P<dot>\.?)[ \t]+(?P<title>[A-Z].{2,68})")
_NEGATION = re.compile(r"\b(?:not|no|never|without|none|neither|nor)\b|n't", re.I)
_USED = re.compile(
    r"\b(?:us(?:e|ed|es|ing)|employ(?:ed|s)?|utili[sz]ed?|leverag(?:e|ed)|assist(?:ed|ance)?|"
    r"help(?:ed|s)?|edit(?:ed|ing)?|proofread(?:ing)?|correct(?:ed|ing|ions?)?|refin(?:e|ed|ing)|"
    r"improv(?:e|ed|ing)|rephras(?:e|ed|ing)|"
    r"rel(?:y|ied)|generat(?:e|ed)|polish(?:ed)?|aid(?:ed)?)\b",
    re.I,
)
_CLAUSE = re.compile(r"[.;:]|,?\s+\b(?:but|however|although|though)\b")
# "not used, except for polishing", "no use beyond grammar": an exception to a denial is a declared use
_EXCEPT = re.compile(
    r"\b(?:except|other than|apart from|aside from|beyond|limited to|(?:only|solely) (?:for|to|as))\b", re.I
)
# words a heading does not contain, and words it does not end on: "3 We then use", "A Large fraction of"
_RUNNING = {
    "we",
    "then",
    "our",
    "is",
    "are",
    "was",
    "were",
    "this",
    "that",
    "which",
    "it",
    "use",
    "used",
    "show",
    "shows",
}
_DANGLING = {"of", "the", "a", "an", "in", "to", "and", "for", "with", "on", "by", "from", "as"}
# lines a PDF repeats on every page, which are not part of what the authors say
_FURNITURE = re.compile(
    r"\d{1,3}|Published as a conference paper at .*|Under review as .*|Preprint\b.*|"
    r"Proceedings of the .*|\d+(?:st|nd|rd|th) (?:International )?Conference on .*",
    re.I,
)


def _titled(heading) -> bool:
    """A heading reads as a title: "Limitations", not the last word of a sentence that wrapped ("limitations.")."""
    return heading.group("title")[0].isupper()


def _is_heading(line: str) -> bool:
    """Does this line start another section?  Text from two-column papers has short lines that begin "A Large
    fraction of ..." or "3 We then ...", so a letter or number in front is not enough: the rest must read as a
    title."""
    line = line.strip()
    if not 3 <= len(line) <= 72:
        return False
    statement = _HEADING.fullmatch(line)
    if _CLOSING.fullmatch(line) or statement and _titled(statement):
        return True
    letters = [c for c in line if c.isalpha()]
    if len(letters) >= 8 and all(c.isupper() for c in letters):  # REFERENCES, A  PROOFS OF THEOREMS
        return True
    section = _SECTION.fullmatch(line)
    if not section or line[-1] in ".,;:":
        return False
    words = section.group("title").split()
    if section.group("dot") or section.group("rest"):
        return True
    lowered = [word.lower().strip(",") for word in words]
    if set(lowered) & _RUNNING or lowered[-1] in _DANGLING:
        return False
    return len(words) <= 3 or all(word[0].isupper() for word in words if len(word) >= 4 and word[0].isalpha())


def _checklist(text: str) -> list:
    """The checklist's items, each read from its own stretch of text so that one unreadable item cannot take the
    next one's answer.  An item counts only if it starts "N. Title" followed shortly by "Question:"."""
    start = text.rfind("Paper Checklist")
    if start < 0:
        return []
    section = text[start:]

    def titled(found) -> bool:  # its title runs straight into its question
        title, question, _ = section[found.end() : found.end() + 320].partition("Question:")
        return bool(question) and not re.search(r"Answer:|Justification:|Guidelines:", title)

    starts = [found for found in _ITEM_START.finditer(section) if 1 <= int(found.group(1)) <= 30 and titled(found)]
    items = {}
    for found, following in zip(starts, starts[1:] + [None]):
        number = int(found.group(1))
        block = section[found.end() : following.start() if following else len(section)]
        title, _, rest = block.partition("Question:")
        answer = _ANSWER.search(rest)
        if number in items or not answer:
            continue
        said = _ANSWERS.get(re.sub(r"[^a-z]", "", (answer.group(1) or answer.group(2)).lower()))
        if said is None:  # "[TODO]", "[Yes/No]": not an answer
            continue
        justification = re.search(r"Justification:\s*(.*?)(?=\n\s*Guidelines:|\Z)", rest[answer.end() :], re.S)
        title = " ".join(line for line in title.split("\n") if not re.fullmatch(r"\s*\d{1,3}\s*", line)).split()
        items[number] = {
            "number": number,
            "title": " ".join(title),
            "answer": said,
            "justification": " ".join((justification.group(1) if justification else "").split())[:600],
        }
    return [items[number] for number in sorted(items)]


def _statements(text: str) -> list:
    """[{"kind", "title", "text"}]: each kind once, the first place it is found outside the checklist."""
    end = text.rfind("Paper Checklist")
    body = text if end < 0 else text[:end]
    found = {}
    for heading in _HEADING.finditer(body):
        kind = next(k for k, _ in _HEADINGS if heading.group(k))
        if kind in found or not _titled(heading):
            continue
        # a numbered section on the use of language models early in a paper is its subject, not a declaration
        if kind == "llm" and heading.group("prefix")[:1].isdigit() and heading.start() < 0.6 * len(body):
            continue
        lines = [
            line
            for line in body[heading.end() : heading.end() + 2500].split("\n")[1:]
            if not _FURNITURE.fullmatch(line.strip())
        ]
        stop = next((number for number, line in enumerate(lines) if _is_heading(line)), len(lines))
        said = " ".join(((heading.group("same_line") or "") + " " + " ".join(lines[:stop])).split())
        if len(said) >= 40:  # a heading with nothing under it is a table-of-contents line
            found[kind] = {"kind": kind, "title": " ".join(heading.group("title").split()), "text": said[:1500]}
    return list(found.values())


def _declares_use(statement: str) -> bool:
    """Does a statement on language models say they were used?  "We used one only to polish the text; it was not
    used for ideas" does; "No language models were used" does not.  Judged clause by clause: a clause that speaks
    of use without negating it settles it, and so does an exception to a denial ("not used, except for
    polishing").  A statement that says neither ("this section is left empty") declares nothing."""
    said = statement[:700]
    if _EXCEPT.search(said):
        return True
    return any(_USED.search(clause) and not _NEGATION.search(clause) for clause in _CLAUSE.split(said))


def read_declarations(text: str) -> dict:
    """What the authors declare in a paper's text: {"checklist", "statements", "flags"}."""
    text = unicodedata.normalize("NFKC", text)  # ligatures ("Justiﬁcation") and odd spaces, as PDFs have them
    checklist, statements = _checklist(text), _statements(text)
    flags = {
        flag
        for item in checklist
        for about, flag, answer in CHECKLIST_FLAGS
        if item["answer"] == answer and re.search(about, item["title"], re.I)
    }
    llm = next((s for s in statements if s["kind"] == "llm"), None)
    if llm and _declares_use(llm["text"]):
        flags.add("llm_declared")
    return {"checklist": checklist, "statements": statements, "flags": [flag for flag in FLAGS if flag in flags]}


# ------------------------------------------------------------------ what has been read, kept
_read = {"stamp": None, "content": {}}  # the declarations file, read again only when it has changed


def stored() -> dict:
    """{paper id: what was read from it}, for the papers read by this version of the reader."""
    stamp = DECLARATIONS.stat().st_mtime_ns if DECLARATIONS.exists() else None
    if stamp != _read["stamp"]:
        content = store.read_json(DECLARATIONS) or {}
        _read.update(
            stamp=stamp, content={paper: read for paper, read in content.items() if read.get("parser") == PARSER}
        )
    return _read["content"]


def known(paper_id: str) -> dict | None:
    """What was read from this paper before, if it was."""
    return stored().get(paper_id)


def _read_paper(paper: dict, pdf_folder: Path) -> dict:
    """LookupError if the paper has no PDF, ValueError if its PDF has no text to read."""
    from pypdf import PdfReader

    pages = PdfReader(str(fetch_pdf(paper, pdf_folder))).pages
    text = "\n".join(page.extract_text() or "" for page in pages)
    if len(text) < MIN_TEXT:
        raise ValueError("this paper's PDF has no text that can be read (it may be a scan)")
    return {"parser": PARSER} | read_declarations(text)


def _keep(read: dict):
    store.update_json(DECLARATIONS, lambda content: content.update(read))


def declarations(paper: dict, pdf_folder: Path) -> dict:
    """What the authors declare in this paper; read from its PDF once and kept.  LookupError if it has no PDF."""
    result = known(paper["id"])
    if result is None:
        result = _read_paper(paper, pdf_folder)
        _keep({paper["id"]: result})
    return result


def read_all(papers: list, log=print, batch=25) -> int:
    """Read the declarations of every paper that has a PDF and has not been read.  The PDFs are downloaded to a
    temporary folder and not kept (a conference is tens of gigabytes).  Returns how many were read."""
    done = stored()
    todo = [paper for paper in papers if paper["id"] not in done]
    log(f"  {len(papers) - len(todo)} papers read before, {len(todo)} to read")
    read, without, failed, waiting = 0, 0, 0, {}
    with tempfile.TemporaryDirectory(prefix="rp-pdfs-") as folder:
        for number, paper in enumerate(todo, start=1):
            try:
                waiting[paper["id"]] = _read_paper(paper, Path(folder))
                read += 1
            except LookupError:
                without += 1
            except Exception as error:  # noqa: BLE001  one bad PDF must not end the job
                failed += 1
                log(f"    {paper['id']}: could not be read ({type(error).__name__})")
            for leftover in Path(folder).glob("*.pdf"):
                leftover.unlink()
            if number % batch == 0 or number == len(todo):
                if waiting:
                    _keep(waiting)
                    waiting = {}
                log(f"    {number}/{len(todo)}")
    log(f"  {read} papers read; {without} have no PDF a program may fetch; {failed} could not be read")
    return read
