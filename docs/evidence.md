# Evidence: from "the model said X" to a link back into the paper

Every statement a feature shows about a paper or a community must carry evidence a user can click to land on the
exact place in the original paper. Three engine modules and one interface component make that true by construction;
every feature that asks Claude about papers goes through them instead of inventing its own.

| piece                                                       | what it does                                                                                   |
| ----------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `apps/engine/src/text/passages.rs`                          | a paper's PDF as addressable text; where a quote is in it; the sentences citing a reference    |
| `apps/engine/src/evidence.rs`                               | `Evidence`, `Claim`, `Cited`, and `check`, which keeps only statements whose quotes were found |
| `apps/engine/src/model/language.rs`                         | the Claude API: JSON answers, prompt caching, budgets, estimates, the ledger                   |
| `apps/web/components/evidence-link.tsx` (`lib/evidence.ts`) | `EvidenceLink`: a citation that opens the reader at the place, with the words marked           |

## The path

```text
PDF ──pdf_pages──> pages ──read_passages──> Passages (pages, sections, paragraphs, sentences, addresses)
                                               │
prompt: system + the papers' text (cached) + question
   │
Client::ask_json ──> the model's answer: statements, each with quotes and the paper each quote is from (Claim<T>)
                                               │
evidence::check(claims, papers, rules) ──> Checked { cited: Vec<Cited<T>>, dropped, rejected, counts }
                                               │
the route returns Cited<T>[] ──> the interface shows each statement with <EvidenceLinks evidence={...} />
                                               │
click ──> PaperReader opens at the address: the Text tab with the words marked, or the PDF at #page=N
```

1. **Read the paper.** `passages_of_pdf(bytes)` (or `pdf_pages` then `read_passages`) gives `Passages`: every page's
   text as rebuilt (running headers, footers and page numbers dropped; hyphenation and column breaks undone), and its
   passages in reading order, each with a `kind` (text, heading, figure or table caption, equation, footnote,
   fragment), a `part` (front, body, appendix, checklist) and an `Address { page, section, paragraph, start,
end_page, end }`. The reference list is in the pages' text but is not a passage. Keep `Passages` by paper id and
   `READER`: an address is only stable for the reader version that made it.
2. **Ask.** Send the model the passages it needs, preferably with their addresses or at least their page, and ask
   for JSON: each statement with `quotes: [{paper, quote}]`, the quote copied word for word. A JSON schema for the
   answer goes in `Form::Json`. The papers' text is the `shared` part of the `Ask` (cached); the question is after it.
3. **Check.** `evidence::check(claims, &papers, rules)` looks for every quote in the paper it names, keeps the
   statements whose quotes were found in at least `rules.papers` distinct papers, and replaces each found quote by
   the paper's own wording at its place. `papers` is the set the feature may cite (a community's members, a run's
   papers, the one paper asked about): a quote naming any other paper is refused.
4. **Show.** Return `Cited<T>` to the interface and draw each piece of evidence with `EvidenceLink` (or a list with
   `EvidenceLinks`). It shows the paper's short label ("Vaswani et al., 2017"), the quote and how it was found on
   hover, opens the reader at the place on click, and offers to find the paper on the graph when it is in the run.

## The rules

- **No unverified quote is ever shown.** `Evidence` can only be made by finding its quote (`check`,
  `Evidence::locate`) or by reading a place of a read paper (`Evidence::at`, for citing sentences). Its
  `established` is `verified` (found in the PDF's text: it has an address) or `abstract` (found in the title or
  abstract of a paper whose PDF has not been read: no page is known, and the link says so). There is no third kind.
- **What is shown is the paper's wording, not the model's.** A quote is found when its letters and digits match the
  paper's after spacing, line breaks, hyphenation, ligatures, accents, quote marks, punctuation and case are set
  aside (`matched: exact`), or with a few letters different (`matched: close`, at most one in twenty, eight in all,
  only for quotes of 20 letters or more; the hover says how many). A quote of fewer than 20 letters and digits
  (`SHORTEST_QUOTE`) is never evidence.
- **A statement whose quotes fail is dropped, not shown bare.** A kept statement shows only its found quotes; the
  others are listed in `Checked::rejected` and counted (`not_in_paper`, `unknown_paper`, `too_short`). A statement
  with found quotes from fewer than `rules.papers` papers goes to `Checked::dropped`. `Rules::ONE_PAPER` is for
  statements about one paper; `Rules::pattern(3)` for "several papers share this" (the overview's old `MIN_CITED`).
  A feature may say how many were left out (`counts.dropped`), never what they said.
- **A quote attributed to the wrong paper fails**: it is looked for only in the paper it names.
- **The statement is a model's reading; the quote is the paper's.** Wherever a statement is shown, it is labelled as
  written by Claude from the papers (CLAUDE.md: what is an estimate or a program's reading says so on hover).

## Costs: estimated before, capped during, recorded after

- **Before.** `language::estimate(model, &planned, counting)` says what a batch is likely to cost and at most, in
  dollars from the prices in `apps/engine/data/prices.json` ("about USD 12 for these 739 papers"). Prompt tokens are
  reckoned from characters: at `CHARS_PER_TOKEN` (3, assumed, so it errs high), or at the ratio
  `Client::calibrate(ask)` measured on one real prompt with the API's token-counting endpoint
  (`POST /v1/messages/count_tokens`). Either way it is an estimate, and the interface must say so
  (`Estimate::counting`). Repeated questions over one shared part are counted as one cache write and then cache reads.
- **During.** Every call takes a `Budget` (`Limit::Dollars` or `Limit::Tokens`) shared by the batch. A call is not
  started when its worst case (its prompt written to the cache, an answer of `max_tokens`) would pass what is left;
  a budget in dollars refuses a model with no known price. Set `max_tokens` to what the answer needs: it is most of
  the worst case.
- **After.** Every call is recorded through a `Recorder` (the library's ledger, or the session's list) with its
  `purpose` and its tokens by how they are billed: full-rate input, output, cache reads, five-minute and one-hour
  cache writes, each at its own price. A call whose answer was refused or cut off is recorded too: it was paid for.
  When a fallback model answered, each model's share is recorded under its own name and price.

## What is cached

- **The prompt prefix, on the API side.** The system prompt and the `shared` part (a paper's text, a community's
  papers) are marked `cache_control: ephemeral` (five minutes); the question comes after the last mark. Keep both
  byte for byte the same across the questions of a batch (no dates or ids in them, papers in a fixed order), ask the
  questions one after another, and check `Answered::tokens.cache_read` on the second call: if it is zero, something
  in the prefix changed. Prefixes under 512 tokens are not cached by the current models.
- **Read papers, in the engine.** `Passages` are kept by paper id and `READER` (reading a PDF takes a moment, and
  stored addresses must keep pointing at the same words). Answers are kept by what produced them (model, papers,
  question), as overviews are: work that was done is not done again. Keep the model's `Claim`s, not `Cited`:
  `Evidence` is never read back from JSON (that would make it unchecked), so a stored answer is checked again when
  it is loaded, which is cheap once the passages are kept.

## Errors

`AskError` is one of `Unavailable` (no key, refused key, unknown model, billing, spend limit, wrong endpoint: stop
the batch and tell the user; the client sends nothing more), `NoAnswer` (rate limit, overload, server error, too
large, refusal, cut off at `max_tokens`, not the JSON asked for: skip this item, the next may work; rate limits are
waited out as `retry-after` says, twice) or `OverBudget` (stop the batch: the user's cap is reached). Messages
never contain the key or the endpoint.

## The routes

| route                           | body                 | answer                                                                         |
| ------------------------------- | -------------------- | ------------------------------------------------------------------------------ |
| `GET /api/papers/<id>/passages` |                      | `PaperPassages` (engine `Passages`), 404 when the paper's PDF cannot be had    |
| `POST /api/papers/<id>/locate`  | `{ "quote": "..." }` | `LocateAnswer`: `{found: true, address, wording, matched}` or `{found: false}` |

Both read the paper's PDF through the library (kept by id) and its `Passages` (kept by id, `READER` and the PDF's
hash): `apps/engine/src/api/passages.rs`. `LocateAnswer::from(passages.locate(quote))` is the second route's answer;
a quote of more than `LONGEST_QUOTE` (5,000) characters, or an empty one, is refused.

## The reader

`PaperReader` takes `at: ReaderTarget { paper, address?, quote? }`; `EvidenceLink` sets it through the workspace's
`EvidenceProvider`. A paper the engine can read opens on its **Text** tab: its passages by section, page boundaries
marked, the cited words marked and scrolled into view; a target with a quote and no address is located first. Each
page boundary has a button that opens the PDF at that page. A paper whose PDF can only be shown, not read, opens
its PDF at `#page=N`. The browser's PDF viewer honours `#page=N` only: it cannot be asked to highlight a passage,
which is why the Text tab is where a link back lands.

## An example

A feature that answers a question about one paper, end to end (engine side):

```rust
use constellate::evidence::{self, Claim, PaperText, Papers, Rules};
use constellate::model::language::{Ask, Budget, Client, Effort, Endpoint, Form, Https, Limit, DEFAULT_MODEL};
use constellate::text::passages::passages_of_pdf;
use constellate::usage::Usd;

#[derive(serde::Deserialize, serde::Serialize)]
struct Answer {
    sentence: String,
}

fn answer(pdf: &[u8], paper: &str, question: &str, library: &constellate::library::Library) -> Result<(), Box<dyn std::error::Error>> {
    let passages = passages_of_pdf(pdf)?;
    // the paper's text with each paragraph's page, so the model can say where it read something
    let text: String = passages
        .passages
        .iter()
        .map(|passage| format!("[p. {}] {}\n", passage.address.page, passage.text))
        .collect();
    let schema = serde_json::json!({
        "type": "object", "additionalProperties": false, "required": ["statements"],
        "properties": {"statements": {"type": "array", "items": {
            "type": "object", "additionalProperties": false, "required": ["statement", "quotes"],
            "properties": {
                "statement": {"type": "object", "additionalProperties": false, "required": ["sentence"],
                              "properties": {"sentence": {"type": "string"}}},
                "quotes": {"type": "array", "items": {"type": "object", "additionalProperties": false,
                    "required": ["paper", "quote"],
                    "properties": {"paper": {"type": "string"}, "quote": {"type": "string"}}}}}}}}
    });
    let transport = Https::default();
    let client = Client::new(&transport, Endpoint::from_environment()?, library);
    let budget = Budget::new(Limit::Dollars(Usd::from_picodollars(500_000_000_000))); // USD 0.50
    let ask = Ask {
        purpose: "questions about a paper",
        model: DEFAULT_MODEL,
        system: "Answer from the paper only. Support every statement with quotes copied word for word.",
        shared: Some(&text),
        question,
        form: Form::Json(&schema),
        effort: Effort::Medium,
        max_tokens: 4_000,
    };
    #[derive(serde::Deserialize)]
    struct Said {
        statements: Vec<Claim<Answer>>,
    }
    let said: Said = client.ask_json(&ask, &budget)?.answer;
    let papers = Papers::from([(paper.to_owned(), PaperText::Read(passages))]);
    let checked = evidence::check(said.statements, &papers, Rules::ONE_PAPER);
    // checked.cited is what the route returns; checked.counts goes to the log
    Ok(())
}
```

And in the interface:

```tsx
{
	answer.map((cited) => (
		<p key={cited.statement.sentence}>
			<span title="Written by Claude from the paper; the quotes are the paper's.">{cited.statement.sentence}</span>{" "}
			<EvidenceLinks evidence={cited.evidence} />
		</p>
	));
}
```

For the citing sentences of feature 2: `citing_sentences(&passages, &read_reference_list(&pages.join("\n")))` gives,
for each reference, its sentences with their addresses; `Evidence::at(paper, &passages, &sentence.address)` makes
the evidence for one without asking the model to quote it.

## What every feature starts from

Four pieces, so that no feature reads PDFs, keeps answers, or runs batches of calls its own way:

| piece                                                       | what it does                                                                               |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `Library::paper_texts(&ids)` / `paper_text(id)` / `citable` | each paper's `PaperReading` (read, no PDF, unreadable, not in the library), PDFs read once |
| `Library::kept` / `keep` / `forget`, `AnswerKey::new`       | the model's raw answers by `Feature` and key; `forget` is a redo button                    |
| `model::batch::estimate_batch` / `ask_all`                  | the cost before, then the calls: kept ones skipped, a few at a time, within a `Budget`     |
| `Job::start_tracked`, `Tracker`, `POST /api/job/cancel`     | "n of m done, x USD so far" in `/api/status`, and a stop button                            |

An extraction table over a community's papers, engine side (`Said { cells: Vec<Claim<Cell>> }` is the answer's shape):

```rust
const PROMPT: u32 = 1; // raise it when the prompt or the answer's shape changes: kept answers are then asked again
let readings = library.paper_texts(&papers)?; // PDFs read on every core the first time, kept after
let texts: Vec<String> = readings.iter().map(|reading| reading.passages().map(as_prompt).unwrap_or_default()).collect();
let items: Vec<Item> = papers.iter().zip(&texts).map(|(paper, text)| Item {
    slot: Slot {
        feature: Feature::Extraction,
        key: AnswerKey::new(Feature::Extraction, DEFAULT_MODEL, PROMPT, &[paper], &columns_json),
        subject: Subject::Paper((*paper).to_owned()),
    },
    prompt: PROMPT,
    ask: Ask { purpose: "extraction table", model: DEFAULT_MODEL, system: SYSTEM, shared: Some(text),
               question: &columns_json, form: Form::Json(&schema), effort: Effort::Low, max_tokens: 2_000 },
    likely_tokens: 600,
}).collect();
// GET .../extraction/estimate: shown before the user starts ("about USD 4 for these 739 papers"); nothing is sent
let estimate = batch::estimate_batch::<Said>(&library, &items, Counting::ASSUMED)?;
// POST .../extraction: one job; its progress and the stop button come with it
engine.job.start_tracked("extraction table", move |tracker| {
    let transport = Https::default();
    let endpoint = engine.language.clone().map_err(|why| why.to_string())?;
    let asking = Asking { library: &library, transport: &transport, endpoint, recorder: &*library, in_flight: IN_FLIGHT };
    let outcomes = batch::ask_all::<Said>(asking, &items, &budget, tracker).map_err(|error| error.to_string())?;
    // ... the result is a count; the table is read with GET, which checks the kept claims as below
});
// GET .../extraction: what is kept, its quotes checked again against the same texts
for ((paper, reading), outcome) in papers.iter().zip(readings).zip(outcomes) {
    let said = match outcome { Outcome::Kept(kept) => kept.answer, Outcome::Answered(new) => new.answer, _ => continue };
    let cite = Papers::from([((*paper).to_owned(), reading.into_text().ok_or(ApiError::NotFound)?)]);
    rows.push(evidence::check(said.cells, &cite, Rules::ONE_PAPER).cited);
}
// the redo button: library.forget(Feature::Extraction, &[Subject::Paper(id), ...])
```

What to know about each piece:

- `paper_texts` takes ids in any order and answers in that order. A paper's PDF is `pdfs/<id>.pdf` in the library's
  folder (`Library::pdf_file`; `keep_pdf` puts one there): nothing here downloads. Several hundred ids at a time;
  passages are held in memory.
- `AnswerKey::new(feature, model, prompt, papers, question)`: `question` is everything else the answer depends on
  (the question, the columns as JSON, the passage's address). The papers are a set; the reader version is added.
  Items of one batch with the same slot are asked once and share the outcome.
- `ask_all` answers one `Outcome` per item in the items' order: `Kept`, `Answered`, `Failed(Failure)` (this one
  only) or `NotStarted(Stopped)` (budget, cancelled, the API unavailable). Items sharing model, system and `shared`
  are asked in turn on one thread so the prompt cache is read; at most `in_flight` calls at once; every call is
  recorded in the ledger; every answer is kept as it arrives. Tests use a fake `Transport`: never the network.
- One job runs at a time (`Busy` otherwise). Work started with `start_tracked` checks `tracker.cancelled()`
  (`ask_all` does) and returns what it has.

The conventions that keep seven features' branches from colliding:

- **Engine**: the feature's code in its own module file (`apps/engine/src/<area>/<feature>.rs`, declared with one
  `pub mod` line), its routes in its own file `apps/engine/src/api/<feature>.rs` with a `table()`, added to
  `routes::table()` with one `table.extend(super::<feature>::table());` line (and one `pub mod` line in `api/mod.rs`).
- **Its `Feature` variant** is already in `library/answers.rs`; use it, and do not rename it (it is stored).
- **A table of its own** only if `kept_answers` cannot hold what it keeps: then one migration appended at the end of
  `library/schema.rs` with a comment naming the feature. Branches will collide on the number; the integrator
  renumbers on merge (a migration is never edited once released).
- **Contracts**: its types appended at the end of `packages/contracts/src/index.ts`, under a banner comment
  `// ---- <feature name> ----`; nothing existing is changed in place. Its calls added at the end of `api` in
  `apps/web/lib/api.ts`.
- **Interface**: its own component files under `apps/web/components/`, mounted with one or two lines where it shows
  (the reader's tabs, a community's sections); its tests in `apps/web/tests/<feature>.test.tsx`.
- **The changelog**: one line under `## [Unreleased]`, `### Added`.

## Measured on real papers

`cargo run --release --example passages_eval -- <PDFs>` on five ICML 2026 and arXiv papers (18 to 36 pages):
75 to 93 % of the letters and digits the PDF reader gives land in passages, 4 to 23 % are the reference list, about
2 % are running headers, footers and page numbers. Of about 65 sentences sampled from each paper's raw PDF text
(page by page, with its line breaks and hyphenation), 96 to 100 % are found, almost all exactly; with two letters
changed, the same share is found as close matches. Sentences of another paper are not found, except boilerplate
that really is in both (the venue's notice). What is missed are "sentences" that run through a figure or a column's
footnotes in the raw text, which the passages put back in reading order.
