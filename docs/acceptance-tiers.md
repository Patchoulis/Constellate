# Acceptance tiers: what each conference calls them

Constellate counts every conference paper in one of three tiers that hold across conferences: **oral** (the top
tier, a talk), **spotlight** (singled out below that) and **poster** (an ordinary acceptance). Conferences do not
name their tiers the same way, and one conference's names change from year to year, so the tier is not read from
the words in a decision: it is looked up in a table, per conference and year, of every decision its list was seen
to use. The table is `TERMS` in `apps/engine/src/sources/conferences/acceptance.rs` and in `apps/backend/rp/tiers.py`; the two
are the same rows, and this page says where they come from.

Checked on 2026-11-26.

## How the lists were read

Each conference in the catalogue with an official list publishes it at
`https://<host>/static/virtual/data/<slug>-<year>-orals-posters.json`. Every list for 2021 to 2026 was asked for
once (a generic User-Agent, three seconds between requests). These answered; the others answered HTTP 404:

| Conference | Years with a list                  | Host                |
| ---------- | ---------------------------------- | ------------------- |
| NeurIPS    | 2021, 2022, 2023, 2024, 2025, 2026 | neurips.cc          |
| ICML       | 2021, 2022, 2023, 2024, 2025, 2026 | icml.cc             |
| ICLR       | 2021, 2022, 2023, 2024, 2025, 2026 | iclr.cc             |
| AISTATS    | 2021, 2022, 2023, 2024, 2025, 2026 | virtual.aistats.org |
| MLSys      | 2021, 2022, 2023, 2024, 2025, 2026 | mlsys.org           |
| CVPR       | 2023, 2024, 2025, 2026             | cvpr.thecvf.com     |
| ICCV       | 2025                               | iccv.thecvf.com     |
| ECCV       | 2024, 2026                         | eccv.ecva.net       |
| WACV       | 2026                               | wacv.thecvf.com     |

An entry of a list has a `decision` and an `eventtype` (`Poster`, `Oral`, `Spotlight`). A paper with a talk is
listed twice, once for its poster and once for its talk; the app reads the poster and recognises the talk as the
same paper by its link or its title. The papers counted below are those papers, each once, as the app reads them.

**The talk matters.** Several lists name a paper's tier only by the kind of its other entry, not by its decision:
CVPR 2025 and ECCV 2024 give no decision at all, and mark their orals only with an `Oral` entry; WACV 2026 and
ECCV 2026 decide `Accept Poster` for everyone and list orals (and ECCV's spotlights) as separate entries; ICML
2026 chose its orals from among its spotlights, whose decision stays `Accept (spotlight)`; AISTATS 2026's oral
entries say `Accept (Oral)` while the same papers' posters say `Accept (Poster)`. So the app now keeps a paper's
decision together with the kind of any other entry the decision does not already name: `Accept Poster; Oral`,
`Accept (spotlight); Oral`, or plain `Oral` when there was no decision. The table has a row for each of these. A
paper's `comment` in the library holds that text.

A talk alone does not make an oral. MLSys gives every accepted paper a talk; ICML 2021 and 2022 gave every paper
a talk, short or long; ICLR 2023 gave talks to some of its "notable-top-25%" papers. The table says, per
conference and year, what the talk means there.

**Tracks.** NeurIPS lists its Datasets and Benchmarks track (Evaluations and Datasets in 2026), its position paper
track, and journal-track posters (JMLR, TMLR, the ML Reproducibility Challenge) in the same list; ICLR and ICML list
journal-track posters and ICLR its blog posts. The track is in the entry's `sourceurl`, which the app does not keep;
the decision's capitalisation sometimes differs between tracks (`Accept (oral)` in the main track, `Accept (Oral)`
in Datasets and Benchmarks, in 2023 and 2024) but not reliably (both tracks wrote `Accept (Poster)` in 2021, and
`Accept (spotlight)` in 2025), so terms are matched without regard to case and the tracks' papers share a row. Each
track's orals and spotlights are chosen by that track's own committee.

## Awards

The decisions in these lists do not name the conferences' own awards. CVPR's "Award Candidate" and best papers,
ICCV's and ECCV's award candidates, NeurIPS's and ICML's outstanding papers are announced separately and are, in
the lists, orals like any other. The only award words seen in a decision are NeurIPS 2023's
`Accept (Outstanding Paper)` and `Accept (Outstanding Paper Honorable Mention)`, and those are awards of the ML
Reproducibility Challenge to reproducibility reports presented as posters at NeurIPS, not NeurIPS's own awards.

So, decided: **an award is shown as an oral with the award named**, not as a tier of its own. The filters, the
graph's sizes and the communities' counts stay on three tiers, and the tag says the award in the conference's
words ("Award Candidate", "Best Paper"), so distinct honours stay apart where the data names them: a term the
table has never seen that contains "award", "best paper" or "outstanding" is counted as an oral and shown as
written. The two Reproducibility Challenge awards keep their own names ("Outstanding paper (Reproducibility
Challenge)", "Honorable mention (Reproducibility Challenge)") and are counted as posters, which is what they were
at NeurIPS.

## A term the table does not know

When a conference and year are not in the table but another year of the same conference used the term, the latest
such year's row is used, and the hover text says so ("read as at NeurIPS 2026, where it was 1.5% of the papers in
its list"). When that row counts the term below what its words say (ICML 2021's "Spotlight" was a poster; NeurIPS
2023's "Outstanding Paper" was a Reproducibility Challenge award), the meaning may have been that year's only: the
lower tier is kept, never promoted, but the term is said to be unknown and collected, so that a maintainer confirms
it. A term no year of the conference used is read from its wording and said to be unknown:

| Its wording contains                                               | Counted as |
| ------------------------------------------------------------------ | ---------- |
| oral, award, best paper, outstanding, long talk, long presentation | oral       |
| spotlight, highlight, notable                                      | spotlight  |
| anything else                                                      | poster     |

The tag then shows the term as written without its "Accept" ("Accept (Best Paper)" shows "Best Paper"), and its
hover text says the tier was read from the wording. Every run lists such terms in its log, with how many papers
have each (`unknown_terms` in both languages), so that a maintainer can add a row. A paper whose list gives no
decision is a poster with no term.

## What is shown

The tag on an oral or a spotlight shows the conference's term ("Highlight", "Notable top 5%", "Long
presentation"); its colour follows Constellate's tier; its hover text says what the term is there and how it is
counted, for example "CVPR 2024's Highlight: 11.9% of the papers in its list; a poster singled out as a highlight.
Constellate counts it as a spotlight." The "Orals" and "Spotlights" chips above the graph and the Settings filter
(all / spotlights and orals / orals only) work on the tiers, and their hover text lists the terms they include. A
run saved before terms were kept shows the tier's generic name ("Oral", "Spotlight").

Shares are of the papers in the conference's list that year, which includes the other tracks and journal-track
posters; the conferences' own figures are usually of the main track's accepted papers, or of submissions.

## The tables

"Papers" after the year is how many papers the list has; "Papers" after the term, how many had that decision
exactly. Where a list wrote one term in two ways, both spellings are given with their counts. "Share" is what the
hover text says, and what the table in the code holds: the share of the list's papers with the same tier, wording
and meaning that year, so the decisions that are one honour with or without a talk listed (ICLR 2023's
notable-top-5%, NeurIPS 2022's featured papers) share one figure. "Shown as" is the tag's text.

### NeurIPS

| Year | Papers | The conference's term (as listed)                     | Papers | Share | What it is                                                                          | Shown as                                      | Our tier  |
| ---- | -----: | ----------------------------------------------------- | -----: | ----: | ----------------------------------------------------------------------------------- | --------------------------------------------- | --------- |
| 2021 |   2334 | `Accept (Oral)`                                       |     55 |  2.4% | presented as a talk                                                                 | Oral                                          | oral      |
| 2021 |   2334 | `Accept (Spotlight)`                                  |    260 | 11.1% | a short spotlight talk                                                              | Spotlight                                     | spotlight |
| 2021 |   2334 | `Accept (Poster)`                                     |   2019 | 86.5% |                                                                                     | Poster                                        | poster    |
| 2022 |   2953 | `Accept; Spotlight`                                   |    515 | 19.8% | a lightning talk in a Featured Papers Panel; decisions named no tier that year      | Featured                                      | spotlight |
| 2022 |   2953 | `Spotlight`                                           |     70 | 19.8% | a lightning talk in a Featured Papers Panel; decisions named no tier that year      | Featured                                      | spotlight |
| 2022 |   2953 | `Accept`                                              |   2323 | 78.7% | decisions named no tier that year                                                   | Accept                                        | poster    |
| 2023 |   3584 | `Accept (oral)` (67), `Accept (Oral)` (10)            |     77 |  2.1% | presented as a talk; main and Datasets and Benchmarks tracks                        | Oral                                          | oral      |
| 2023 |   3584 | `Accept (spotlight)` (378), `Accept (Spotlight)` (22) |    400 | 11.2% | main and Datasets and Benchmarks tracks                                             | Spotlight                                     | spotlight |
| 2023 |   3584 | `Accept (poster)` (2773), `Accept (Poster)` (311)     |   3084 | 86.0% |                                                                                     | Poster                                        | poster    |
| 2023 |   3584 | `Accept`                                              |     21 |  0.6% | ML Reproducibility Challenge reports                                                | Accept                                        | poster    |
| 2023 |   3584 | `Accept (Outstanding Paper)`                          |      1 |  0.0% | an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster | Outstanding paper (Reproducibility Challenge) | poster    |
| 2023 |   3584 | `Accept (Outstanding Paper Honorable Mention)`        |      1 |  0.0% | an award of the ML Reproducibility Challenge, not of NeurIPS; presented as a poster | Honorable mention (Reproducibility Challenge) | poster    |
| 2024 |   4537 | `Accept (oral)` (61), `Accept (Oral)` (11)            |     72 |  1.6% | presented as a talk; main and Datasets and Benchmarks tracks                        | Oral                                          | oral      |
| 2024 |   4537 | `Accept (spotlight)` (326), `Accept (Spotlight)` (56) |    382 |  8.4% | main and Datasets and Benchmarks tracks                                             | Spotlight                                     | spotlight |
| 2024 |   4537 | `Accept (poster)` (3648), `Accept (Poster)` (419)     |   4067 | 89.6% |                                                                                     | Poster                                        | poster    |
| 2025 |   5858 | `Accept (oral)` (84), `Accept (Oral)` (9)             |     93 |  1.6% | presented as a talk; main, Datasets and Benchmarks, and position paper tracks       | Oral                                          | oral      |
| 2025 |   5858 | `Accept (spotlight)`                                  |    743 | 12.7% | main and Datasets and Benchmarks tracks                                             | Spotlight                                     | spotlight |
| 2025 |   5858 | `Accept (poster)` (4957), `Accept (Poster)` (14)      |   4971 | 84.9% |                                                                                     | Poster                                        | poster    |
| 2025 |   5858 | `Accept`                                              |     31 |  0.5% | position paper track                                                                | Accept                                        | poster    |
| 2026 |   9110 | `Accept (oral)` (128), `Accept (Oral)` (5)            |    133 |  1.5% | presented as a talk; main, Evaluations and Datasets, and position paper tracks      | Oral                                          | oral      |
| 2026 |   9110 | `Accept (spotlight)` (322), `Accept (Spotlight)` (11) |    333 |  3.7% | main, Evaluations and Datasets, and position paper tracks                           | Spotlight                                     | spotlight |
| 2026 |   9110 | `Accept (poster)`                                     |   8414 | 92.4% |                                                                                     | Poster                                        | poster    |
| 2026 |   9110 | `Accept`                                              |    121 |  1.3% | position paper track                                                                | Accept                                        | poster    |

### ICML

| Year | Papers | The conference's term (as listed)               | Papers | Share | What it is                                                                           | Shown as           | Our tier  |
| ---- | -----: | ----------------------------------------------- | -----: | ----: | ------------------------------------------------------------------------------------ | ------------------ | --------- |
| 2021 |   1183 | `Oral`                                          |    166 | 14.0% | a long talk                                                                          | Oral               | oral      |
| 2021 |   1183 | `Spotlight`                                     |   1017 | 86.0% | in 2021 every paper without a long talk gave a short spotlight talk: not singled out | Spotlight          | poster    |
| 2022 |   1233 | `Accept for Long Presentation; Oral`            |    118 |  9.6% | a long talk                                                                          | Long presentation  | oral      |
| 2022 |   1233 | `Accept for Short Presentation; Spotlight`      |   1115 | 90.4% | every other accepted paper gave a short talk: not singled out                        | Short presentation | poster    |
| 2023 |   1865 | `Accept (Oral & Poster)`                        |    155 |  8.3% | presented as a talk and a poster                                                     | Oral               | oral      |
| 2023 |   1865 | `Accept (Poster)`                               |   1710 | 91.7% |                                                                                      | Poster             | poster    |
| 2024 |   2634 | `Accept (Oral)`                                 |    144 |  5.5% | presented as a talk                                                                  | Oral               | oral      |
| 2024 |   2634 | `Accept (Spotlight)`                            |    191 |  7.3% | a spotlight poster                                                                   | Spotlight          | spotlight |
| 2024 |   2634 | `Accept (Poster)`                               |   2299 | 87.3% |                                                                                      | Poster             | poster    |
| 2025 |   3339 | `Accept (oral)`                                 |    120 |  3.6% | presented as a talk                                                                  | Oral               | oral      |
| 2025 |   3339 | `Accept (spotlight poster)`                     |    224 |  6.7% |                                                                                      | Spotlight poster   | spotlight |
| 2025 |   3339 | `Accept (poster)` (2987), `Accept (Poster)` (8) |   2995 | 89.7% |                                                                                      | Poster             | poster    |
| 2026 |   6629 | `Accept (spotlight); Oral`                      |    168 |  2.5% | chosen for a talk from among the spotlights                                          | Oral               | oral      |
| 2026 |   6629 | `Oral`                                          |      1 |  0.0% | a talk listed without a decision                                                     | Oral               | oral      |
| 2026 |   6629 | `Accept (spotlight)`                            |    406 |  6.1% | main and position paper tracks                                                       | Spotlight          | spotlight |
| 2026 |   6629 | `Accept (regular)`                              |   5986 | 90.3% |                                                                                      | Regular            | poster    |
| 2026 |   6629 | `Accept (Poster)`                               |      7 |  0.1% | journal-track and other invited posters                                              | Poster             | poster    |

### ICLR

| Year | Papers | The conference's term (as listed)                | Papers | Share | What it is                                                 | Shown as        | Our tier  |
| ---- | -----: | ------------------------------------------------ | -----: | ----: | ---------------------------------------------------------- | --------------- | --------- |
| 2021 |    860 | `Oral`                                           |     53 |  6.2% | presented as a talk                                        | Oral            | oral      |
| 2021 |    860 | `Spotlight`                                      |    114 | 13.3% | a short spotlight talk                                     | Spotlight       | spotlight |
| 2021 |    860 | `Poster`                                         |    693 | 80.6% |                                                            | Poster          | poster    |
| 2022 |   1095 | `Accept (Oral)`                                  |     55 |  5.0% | presented as a talk                                        | Oral            | oral      |
| 2022 |   1095 | `Accept (Spotlight)`                             |    174 | 15.9% | a short spotlight talk                                     | Spotlight       | spotlight |
| 2022 |   1095 | `Accept (Poster)`                                |    866 | 79.1% |                                                            | Poster          | poster    |
| 2023 |   1584 | `Accept: notable-top-5%; Oral`                   |     69 |  5.7% | the conference's name that year for its orals              | Notable top 5%  | oral      |
| 2023 |   1584 | `Accept: notable-top-5%`                         |     22 |  5.7% | the conference's name that year for its orals              | Notable top 5%  | oral      |
| 2023 |   1584 | `Accept: notable-top-25%; Oral`                  |    179 | 17.7% | the conference's name that year for its spotlights         | Notable top 25% | spotlight |
| 2023 |   1584 | `Accept: notable-top-25%`                        |    101 | 17.7% | the conference's name that year for its spotlights         | Notable top 25% | spotlight |
| 2023 |   1584 | `Accept: poster`                                 |   1203 | 75.9% |                                                            | Poster          | poster    |
| 2023 |   1584 | `Accept (Poster)`                                |     10 |  0.6% | journal-track and other invited posters                    | Poster          | poster    |
| 2024 |   2296 | `Accept (oral)`                                  |     86 |  3.7% | presented as a talk                                        | Oral            | oral      |
| 2024 |   2296 | `Accept (spotlight)`                             |    367 | 16.0% | a spotlight poster                                         | Spotlight       | spotlight |
| 2024 |   2296 | `Accept (poster)` (1807), `Accept (Poster)` (36) |   1843 | 80.3% |                                                            | Poster          | poster    |
| 2025 |   3830 | `Accept (Oral)`                                  |    216 |  5.6% | presented as a talk                                        | Oral            | oral      |
| 2025 |   3830 | `Accept (Spotlight)`                             |    380 |  9.9% |                                                            | Spotlight       | spotlight |
| 2025 |   3830 | `Accept (Poster)`                                |   3185 | 83.2% |                                                            | Poster          | poster    |
| 2025 |   3830 | `Accept`                                         |     49 |  1.3% | the blog post track                                        | Blog post       | poster    |
| 2026 |   5468 | `Accept (Oral)`                                  |    224 |  4.1% | presented as a talk; there was no spotlight tier that year | Oral            | oral      |
| 2026 |   5468 | `Accept (Poster)`                                |   5244 | 95.9% |                                                            | Poster          | poster    |

### AISTATS

| Year | Papers | The conference's term (as listed) | Papers | Share | What it is          | Shown as | Our tier |
| ---- | -----: | --------------------------------- | -----: | ----: | ------------------- | -------- | -------- |
| 2021 |    455 | `Oral`                            |     48 | 10.5% | presented as a talk | Oral     | oral     |
| 2021 |    455 | `Poster`                          |    407 | 89.5% |                     | Poster   | poster   |
| 2022 |    492 | `Accept (Oral)`                   |     44 |  8.9% | presented as a talk | Oral     | oral     |
| 2022 |    492 | `Accept (Poster)`                 |    448 | 91.1% |                     | Poster   | poster   |
| 2023 |    496 | `Accept (Oral)`                   |     32 |  6.5% | presented as a talk | Oral     | oral     |
| 2023 |    496 | `Accept (Poster)`                 |    464 | 93.5% |                     | Poster   | poster   |
| 2024 |    555 | `Accept (Oral)`                   |     48 |  8.6% | presented as a talk | Oral     | oral     |
| 2024 |    555 | `Accept (Poster)`                 |    507 | 91.4% |                     | Poster   | poster   |
| 2025 |    591 | `Accept (Oral)`                   |     37 |  6.3% | presented as a talk | Oral     | oral     |
| 2025 |    591 | `Accept (Poster)`                 |    546 | 92.4% |                     | Poster   | poster   |
| 2026 |    576 | `Accept (Poster); Oral`           |      8 |  1.4% | presented as a talk | Oral     | oral     |
| 2026 |    576 | `Accept (Poster)`                 |    568 | 98.6% |                     | Poster   | poster   |

### MLSys

| Year | Papers | The conference's term (as listed) | Papers |  Share | What it is                                      | Shown as                | Our tier |
| ---- | -----: | --------------------------------- | -----: | -----: | ----------------------------------------------- | ----------------------- | -------- |
| 2021 |     52 | `Accept; Oral`                    |     52 | 100.0% | every MLSys paper gives a talk: not singled out | Accept                  | poster   |
| 2022 |     51 | `Accept; Oral`                    |     51 | 100.0% | every MLSys paper gives a talk: not singled out | Accept                  | poster   |
| 2023 |     46 | `Accept`                          |     46 | 100.0% |                                                 | Accept                  | poster   |
| 2024 |     37 | `Accept`                          |     33 |  89.2% |                                                 | Accept                  | poster   |
| 2024 |     37 | `Conditional Accept`              |      4 |  10.8% |                                                 | Conditional accept      | poster   |
| 2025 |     61 | `Accept`                          |     48 |  78.7% |                                                 | Accept                  | poster   |
| 2025 |     61 | `Accept with shepherding`         |     13 |  21.3% |                                                 | Accept with shepherding | poster   |
| 2026 |    171 | `Conditional Accept; Oral`        |    135 |  78.9% | every MLSys paper gives a talk: not singled out | Conditional accept      | poster   |

### CVPR

| Year | Papers | The conference's term (as listed) | Papers |  Share | What it is                                                    | Shown as  | Our tier  |
| ---- | -----: | --------------------------------- | -----: | -----: | ------------------------------------------------------------- | --------- | --------- |
| 2023 |   2359 | `Accept`                          |   2359 | 100.0% | the list does not name 2023's highlights or orals             | Accept    | poster    |
| 2024 |   2716 | `Accept: Oral`                    |     90 |   3.3% | presented as a talk                                           | Oral      | oral      |
| 2024 |   2716 | `Accept: Poster (Highlight)`      |    324 |  11.9% | a poster singled out as a highlight                           | Highlight | spotlight |
| 2024 |   2716 | `Accept: Poster`                  |   2302 |  84.8% |                                                               | Poster    | poster    |
| 2025 |   2871 | `Oral`                            |     95 |   3.3% | presented as a talk; the list does not name 2025's highlights | Oral      | oral      |
| 2026 |    199 | `Accept (Oral)`                   |      8 |   4.0% | presented as a talk                                           | Oral      | oral      |
| 2026 |    199 | `Accept (Highlight)`              |     18 |   9.0% | a poster singled out as a highlight                           | Highlight | spotlight |
| 2026 |    199 | `Accept (Poster)`                 |     91 |  45.7% |                                                               | Poster    | poster    |

### ICCV

| Year | Papers | The conference's term (as listed) | Papers | Share | What it is                          | Shown as  | Our tier  |
| ---- | -----: | --------------------------------- | -----: | ----: | ----------------------------------- | --------- | --------- |
| 2025 |   2701 | `oral`                            |     64 |  2.4% | presented as a talk                 | Oral      | oral      |
| 2025 |   2701 | `highlight`                       |    263 |  9.7% | a poster singled out as a highlight | Highlight | spotlight |
| 2025 |   2701 | `poster`                          |   2374 | 87.9% |                                     | Poster    | poster    |

### ECCV

| Year | Papers | The conference's term (as listed) | Papers | Share | What it is             | Shown as  | Our tier  |
| ---- | -----: | --------------------------------- | -----: | ----: | ---------------------- | --------- | --------- |
| 2024 |   2387 | `Oral`                            |    200 |  8.4% | presented as a talk    | Oral      | oral      |
| 2026 |   2834 | `Accept Poster; Oral`             |     28 |  1.0% | presented as a talk    | Oral      | oral      |
| 2026 |   2834 | `Accept Poster; Spotlight`        |    135 |  4.8% | a short spotlight talk | Spotlight | spotlight |
| 2026 |   2834 | `Accept Poster`                   |   2671 | 94.2% |                        | Poster    | poster    |

### WACV

| Year | Papers | The conference's term (as listed) | Papers | Share | What it is          | Shown as | Our tier |
| ---- | -----: | --------------------------------- | -----: | ----: | ------------------- | -------- | -------- |
| 2026 |    831 | `Accept Poster; Oral`             |     85 | 10.2% | presented as a talk | Oral     | oral     |
| 2026 |    831 | `Accept Poster`                   |    746 | 89.8% |                     | Poster   | poster   |

## Checked against the conferences' own pages

What the data shows was compared with what the conferences say. Where only a secondary source was found, it is
named as such.

- **CVPR 2024**: 11,532 submissions, 2,719 accepted, "only 3.3%" of accepted papers slotted for orals
  ([CVPR 2024 news: wrap release](https://cvpr.thecvf.com/Conferences/2024/News/Wrap_Release),
  [CVPR 2024 news: orals](https://cvpr.thecvf.com/Conferences/2024/News/Oral_Papers)). The list: 2,716 papers, 90
  orals (3.3%), 324 highlights (11.9%). The accepted papers page marks highlights with an award icon titled
  "Highlight" ([accepted papers](https://cvpr.thecvf.com/Conferences/2024/AcceptedPapers)).
- **ICLR 2023**: a RIKEN AIP announcement gives the tiers as notable-top-5% ("traditionally called Accept (oral)")
  and notable-top-25% ("traditionally called Accept (spotlight)"), at 1.6% and 8.0% of 4,966 submissions
  ([RIKEN AIP](https://aip.riken.jp/?p=14485), secondary). The list: 91 and 280 of 1,584 papers.
- **ICLR 2026**: the ICLR 2026 fact sheet lists 224 oral presentations
  ([fact sheet](https://media.iclr.cc/Conferences/ICLR2026/ICLR2026_Fact_Sheet.pdf)) and the ICLR blog 5,355
  accepted papers ([retrospective](https://blog.iclr.cc/2026/03/31/a-retrospective-on-the)); the list has 224
  orals and no spotlight decision at all. No ICLR page was found that says in words that there was no spotlight
  tier.
- **ICML 2022**: 118 long and 1,117 short presentations (a secondary report:
  [Tencent Cloud](https://cloud.tencent.com/developer/article/2200118)); RIKEN AIP lists its papers as "Long talk"
  and "Short talk" ([RIKEN AIP](https://aip.riken.jp/?p=11976)). The list: 118 and 1,115.
- **ICML 2026**: RIKEN AIP gives 536 spotlight posters of 23,918 submissions (2.2%) and 6,352 accepted
  ([RIKEN AIP](https://aip.riken.jp/news/icml2026/), secondary); the main track's spotlights in the list are 536.
- **NeurIPS 2022**: the NeurIPS blog says "Oral presentations all take place in the virtual week" and that
  "selected papers are presented in a panel format" ([NeurIPS blog, 2022](https://blog.neurips.cc/category/2022-conference/)).
  The list's `Spotlight` entries are those papers' lightning talks in "Featured Papers Panels"; every decision is
  plain `Accept`.

## Not confirmed from a primary source

- **NeurIPS 2022**: whether the papers with a lightning talk in a Featured Papers Panel (about 18% of the list) are
  what NeurIPS called its orals, or a wider selection. They are counted as spotlights ("Featured"), since their
  share is a spotlight's; the decisions name no tier that year.
- **ICML 2021**: that "Spotlight" was every paper without a long talk is read from the list (1,017 of 1,183
  papers, all with the decision `Spotlight`, and the other 166 `Oral`); no ICML 2021 page was found that says so.
  Counting them as spotlights, as the old reading did, would have made 86% of the conference spotlights.
- **ICML 2026**: that its orals were chosen from its spotlights is read from the list (every oral entry's decision
  is `Accept (spotlight)`).
- **MLSys**: that every paper gives a talk is read from the lists (2021, 2022 and 2026 list a talk for every
  paper; 2023 to 2025 list none). "Conditional accept" and "Accept with shepherding" are review outcomes, not tiers.
- **CVPR 2023 and 2025**: CVPR had highlights both years, but the lists do not name them (2023 has no orals
  either), so those papers are posters here. **CVPR 2026**'s list had only 199 papers when read, so its shares are
  of a partial list.
- **ECCV 2024, ECCV 2026, ICCV 2025, WACV 2026, AISTATS**: the shares are the lists'; no figures from the
  conferences' own pages were found to compare them with. WACV's two rounds of review do not show in its list.
