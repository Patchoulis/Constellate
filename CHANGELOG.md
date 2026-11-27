# Changelog

What changed in Constellate, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Runs are made by the app's own engine: while one runs, it says which step it is on and how far it has got
  ("embedding 3,120 of 12,487 papers", the embedding model's download in megabytes), it can be stopped and
  started again without fetching or embedding anything twice, and the runs and library made before open as they
  were.
- The AI-text detector can be trained with the language model chosen in Settings, after its cost is shown, within
  the budget accepted, and resumed without paying twice; it says which models wrote the text it learnt from, and a
  detector already trained by an earlier version is used as it is.
- A paper's PDF is fetched when it is opened and not yet in the library (from arXiv or the conference's
  proceedings), so its Text tab, citations and the features that read papers work for it too; when it cannot be
  fetched, the reader says why. The Citations tab can now read a paper's reference list.
- The reader's PDF of a paper and every other PDF are read in a separate process: a damaged or hostile PDF is
  marked unreadable and can no longer stop the app.
- The paper reader has a Text tab showing the paper's text section by section, and citations open it at the exact
  place they quote, with the words highlighted; quotes a model gives as evidence are only shown once they have been
  found in the paper they are attributed to.
- In the Text tab, select words (or use a passage's button) to have a paragraph, an equation or a table's caption
  explained by the model: the cost is shown before anything is sent, every statement about the paper links to the
  paper's own words, general background is marked as not from the paper, the sentences where the paper defines the
  symbols and terms used are shown for free, explanations are kept so reopening one costs nothing, and "Find
  related" lists passages of nearby papers in the run that share its words, at no cost.
- The paper reader has a Claims tab: the paper's central claims as a tree, with the findings and evidence
  (experiments, tables, figures, theorems) the paper offers for each, every one linked to the paper's own words; the
  points of its OpenReview reviews placed on the claims they are about, each with the reviewer's words beside the
  paper's, how many reviewers challenged or supported each claim with their ratings on the venue's own scale, a
  filter for contested claims, points that could not be placed listed apart, and the model's own reading of possible
  gaps shown last. The cost is estimated before anything is asked.
- The paper reader has a Weaknesses section: the reviewers' own weaknesses, questions and limitations point by
  point, with each reviewer's rating and confidence and any reply of the authors, each point opening on the
  reviewer's words; and, on request with its cost estimated first, the model's reading of the paper (where each
  point applies, what the paper says about it, and weaknesses the reviewers did not raise), every statement linked
  to the paper's own words, kept apart and never a score or verdict.
- A Citations tab in the reader shows a paper's citation network: the works it cites and the papers that cite it,
  laid out by year with the links between them, and as lists you can filter and sort. It is built from the
  reference lists of the papers Constellate has read, so "cited by" counts only those papers. For each paper of
  your library a paper cites and each paper citing it, it shows the sentences where it happens (with the sentence
  before and after, the section and whether it is cited alone), each linking to the place in the citing paper; this
  needs no API key. On request, after an estimate and within a cap, the model reads what kind of citation each is
  (uses, data or benchmark, baseline, extends, supports, contrasts, mention), always shown beside the sentence and
  only counted when the words that show it are found there. Communities can be sorted by how often they are cited
  and by how many works they cite, and list the papers they most use as a baseline, contrast, or take data from.
- Opening a paper's Reviews tab now keeps the whole discussion (each review by section, rebuttals, the meta-review
  and the decision), shows the paper's mean, range and confidence-weighted mean on its venue's own scale, and adds a
  Reviews section to each community: how many of its papers have reviews read, ratings per conference and year, the
  papers reviewers disagreed on most, and what reviewers most often praise and criticise, each linked to the
  reviewer's words.
- Ask a community (in its panel) or the whole run (the "Ask the run" button over the graph) a question: the passages
  of its papers that talk about it are found first, for free, each linked to the place in the paper; then, after you
  see what it would cost and click, the model answers in statements each followed by the quotes it rests on.
  Statements whose quotes are not found in the papers are left out and counted, "the papers do not say" is shown as
  an answer, and past questions are listed per run, reopened at no cost, asked again or deleted.
- Each community has an Agree / disagree section: ask a yes/no question of its papers (or start from a term its
  papers keep using, or a question the model proposes from their abstracts), see the cost before anything is sent,
  ask the ten most central papers first and the rest on a second click, and see the papers grouped as supporting,
  mixed (with the condition), contradicting or not addressing it, each with the paper's own words, counted by
  acceptance tier and by year; placements whose quotes are not found in the paper are shown apart and never counted,
  and questions are kept so they can be reopened without paying again.
- A Table section for each community and for the marked papers: one row per paper, one column per question (task,
  method, datasets, headline result, baselines, compute, code and data, limitations, or your own), every value shown
  beside the paper's own words and "not stated in the paper" when it does not say. It is run first on the ten most
  central papers, with the estimated cost shown before and the real cost after, then on the rest with a second
  click; it can be sorted, filtered and saved as CSV with every quote and page.
- Names, descriptions and overviews can be written by other providers' models than Claude: OpenAI, Google Gemini,
  OpenRouter, xAI, Mistral, DeepSeek, Groq and a dozen more, or a model on your own computer through Ollama, LM
  Studio or any OpenAI-compatible server. The model menu at the bottom of the sidebar groups them by provider, says
  which ones need a key and which variable to set, takes a model id typed in where the provider allows it, and
  offers the effort where the model has one; the cost line names the provider and model and says "cost not known"
  rather than 0 when a price is not published, and "no API cost" for a model on your own computer. Settings list
  every provider, whether its key is set, where it is reached and what it cannot do.
- Mark papers to keep them in view: right-click a paper on the graph (or use the star in the reader and in a
  community's list) and it gets a yellow ring with its closest papers always linked and named; the Marked tab lists
  a run's marked papers, and marks are remembered for each run. While one community is in focus, only the marked
  papers inside it are shown.
- A Recenter button on the graph brings back what you were looking at: the open paper, else the focused community,
  else the whole graph. The graph glides there instead of jumping, as it does when a community is focused or a
  paper opened; panels slide in and labels fade in. None of it moves for someone who asked their system for less
  motion.
- The runs sidebar, the communities panel, the paper reader, the overview and the activity log can be resized by
  dragging their edge (or with the arrow keys once the edge is focused); double-click an edge to put it back, and
  the sizes are remembered.
- The engine uses any maker's graphics card: through DirectML on Windows and through Vulkan (WebGPU) on Linux, with
  the processor as the fallback. It is built and tested on Linux as well as Windows.
- The engine has the model prices and the exchange rates built in, with the day each rate is from: once it
  replaces the backend, the app no longer asks the internet for a rate. A weekly check compares the rates with the
  European Central Bank's and, when one has moved by more than 1%, prepares a new version with the new rates.
- The engine keeps the library in one small database on your computer instead of many loose files, so an
  interrupted write can no longer leave it half-saved, and an existing library is moved in without losing anything.
  A conference whose site did not answer is now fetched again later instead of being counted as downloaded, and
  papers whose PDF could not be had or read are remembered as such.
- `npm run setup` (or `make setup`) gets a fresh clone ready to work on in one command.

### Changed

- Naming a run's communities and writing a community's overview show what they would cost before anything is sent
  (in the run form, and on the overview's button); each point of an overview is shown only with quotes found in at
  least three of the papers it names, in the papers' own words, and the overview says how many points were left out.
- The engine computes on every core: the similarity graph, the communities, nearest neighbours and citation figures
  are worked out several times faster with the same results, PDFs are read side by side, and downloads run a few at
  a time within each site's limits (arXiv still one request every three seconds). `CONSTELLATE_THREADS` caps the
  threads it uses.
- The redo buttons say what they cover ("Embed all again"), a fourth redoes all of it, and every run in the sidebar
  has its own button to redo all of that run.
- The installed app keeps its library in the system's folder for local, unsynced data (the local application-data
  folder on Windows, `~/.local/share` on Linux): a library in a synced folder can be corrupted while the app is open.
- A paper's acceptance tag uses the conference's own word for it ("Highlight", "Notable top 5%", "Long
  presentation") and says on hover what that meant at that conference that year and whether it counts as an oral or
  a spotlight. Tiers are now looked up per conference and year from what each conference's list was seen to use,
  which fixes ICML 2021 papers shown as spotlights (there every ordinary paper was called a spotlight) and finds the
  orals of CVPR 2025, ECCV, WACV, AISTATS 2026 and ICML 2026, which their lists name only by the talk. For a
  conference downloaded before, use Settings, "Download again", to get those orals; runs saved before keep the tiers
  they were saved with, under the tier's generic name, until they are run again.
- The backend is being rewritten in Rust (`apps/engine`), organised by subject; the Python backend stays until the
  Rust one does everything it does.
- Moving around the graph, and pointing at papers, is smooth again with the whole run in view: a frame takes a few
  milliseconds instead of about 30 with 12,500 papers; the app also opens its window a little sooner.

### Fixed

- A quote a model gives as evidence is no longer accepted when it differs from the paper by a whole word, a number
  or a negation (a quote that leaves out the paper's "not" was found as a close match); only slips inside words
  are still allowed.
- A setting the backend refuses (a number of communities out of range, say) now says why, in the backend's
  words, under the list of runs until that setting is saved, instead of quietly going back; a failed rename or
  delete of a run says why too.
- "Try again" after reading a paper's declarations or its reference list failed now reads it again.
- An open overview is asked for again when the language model is changed in Settings, so the previous model's
  overview is no longer shown under the new one.
- A community's reviews and other sections no longer show the previous community's figures while their own load,
  and a reload that fails keeps what was shown beside the error.
- Without the engine, the reader's Text and Citations tabs say that nothing is available instead of "not found".
- The graph stays sharp and correctly placed when the window moves to a screen with another pixel density, and a
  run with no papers says so instead of drawing nothing.

- The model features (Weaknesses, Agree / disagree, the Table, Explain, Claims, citation kinds and Ask) now work with whichever provider is chosen, as they do with Claude: the cost shown before asking is that provider's, the request goes to that provider with its own model, a model with no published price (a typed id, a model on your own computer) runs within the tokens shown instead of being refused, no dollar figure is shown for it, answers kept for one provider are not taken for another's, and a job is refused if the provider or model changed since its cost was shown.

- A finished feature no longer reopens the last run, and the run that finished before the page was opened is no longer opened again at every load.

- A damaged settings file no longer makes the engine fall back to the Claude API without a word: it says the settings cannot be read. Two windows changing the provider and the model at once can no longer store a model of the wrong provider.

- Redoing a cited answer asked with an older version of the app now forgets it; redoing an explanation says whether there was one to forget.

- A model's answer and what it cost are written together, so that a crash can no longer leave a call paid for whose answer is lost.

- A paper listed twice when reading citing sentences is no longer reported as not in the library the second time.

- The installed app's first start no longer stops with "Missing expected target directory for Python minor version
  link" when Constellate is opened from inside another packaged Windows app (from a terminal in one, say).
- Marked papers and panel sizes are kept between launches: the interface is served on the same port each time. Ending
  the app from the Task Manager, or a crash, no longer leaves the interface's server running in the background, where
  it held that port, and Ctrl+C in the terminal of `npm run app` stops both servers.
- After the model is changed in the settings, a community's overview no longer shows the one the previous model
  wrote as if it were current: it can be written by the model now chosen.
- The new engine now serves the reader's Text tab and finds the words a citation quotes; each paper's PDF is read
  once and kept, and is read again only when the PDF or the way papers are read changes.
- A Claude API key (and address) set in a `.env` file beside the new engine is now used, like its other settings;
  the key is never shown, logged or passed on to another program.
- Reviewers' scores on older OpenReview pages (reviews signed "AnonReviewer2", as ICLR 2021 and earlier sign them)
  are now counted review by review, as the Reviews tab shows them, instead of being read as a guess from the whole
  page.
- A rating scale is said to be the venue's published one only where it was found in the venue's own reviewer
  guidelines (NeurIPS 2022 to 2025, ICML 2024 and 2025); every ICLR scale, and older ICML ones, are now shown as
  assumed.
- What asking the model would cost is worked out again as soon as another provider or model is chosen, and an old
  figure is never shown beside a button that would accept it; a double click starts one job, not two.
- A job asking the model keeps being shown when its section is closed and opened again (another reader tab,
  another community and back), instead of offering to ask again; a job running elsewhere is said, a job that
  stopped early says how many papers were not asked, and an engine that stops answering is asked less often.
- Forgetting an answer the model was paid for (a paper's weaknesses, its claims, an explanation, a question) now
  asks first and says why when it fails; a refused setting is said beside it in the settings dialog.
- "List the server's models" for an OpenAI-compatible endpoint offers only ids that can be used, and says when the
  server is down, slow or answers something else.

### Security

- Vulnerabilities can be reported privately (`SECURITY.md`). The code is scanned by CodeQL, a pull request that
  brings in a vulnerable dependency is refused, and nothing reaches `main` except through a pull request whose
  checks pass.
- The app's own Python is now set up with an exact, checked version of every package (a lock), instead of whatever
  was newest the day it was first set up and then kept: a computer set up earlier is brought to these versions the
  next time the app starts. On a computer with an NVIDIA graphics card this replaces the setuptools 78.1.0 that came
  with PyTorch's CUDA build (CVE-2025-47273, CVE-2026-59890, GHSA-xmj3-9gf7-h52w) with 84.2.1.
- The engine's password check uses ctutils 0.4.4, whose comparisons are constant-time (RUSTSEC-2026-0385), and its
  tests time 0.3.47 (RUSTSEC-2026-0009, CVE-2026-25727). The installer is built with minimatch 11.0.1
  (GHSA-g3hv-75wg-m249).
- Every dependency is now checked against the published advisories on each change and every week, and a new
  advisory fails the check until it is dealt with.

- Pages shown in the reader can no longer send anything to the app's own interface, its API or the engine on this
  computer, nor to the local network, nor open files: every request such a page makes is checked, not only the
  frames it opens.
- The reader is closed off further: a page in it can no longer show dialogs that look like the app's own, make the
  "not a paper site" question appear without your click (or again and again), keep a service worker running after
  it is gone, keep the window from closing, or reach this computer or other sites through calls between browsers
  (WebRTC) or WebTransport; a link in a paper's PDF opens in your browser rather than inside the app's window; no
  page is given a certificate of yours; the app no longer listens on your network (which made Windows ask whether
  "Electron" may use it); and the app's API refuses requests sent to the interface under another site's name.
- The new engine answers only the app itself: every request must carry a secret token and be addressed to the
  engine, and pages of other sites are refused. Run on another machine than the app, it requires HTTPS, and warns
  loudly if HTTPS is turned off.
- The engine can run as your own server, used from elsewhere through a browser, and then only you get in: you sign
  in with Google (or any OpenID Connect provider) as the one account the server is set up for, or with a password
  that is set once with a code the server prints when it first starts and kept only as a strong hash. Every page and
  request of the server needs that sign-in; sessions end after two hours without use or twelve hours in all, and
  you can sign out on every device at once. Requests from other sites are refused, sign-in attempts are limited, and
  everything secret is read from the environment or `.env` only. When the window is connected to such a server, the
  embedding model is the server's to decide and is no longer offered in Settings.
- A web site open in your browser while Constellate runs can no longer read your library, runs and settings by
  pointing a name of its own at your computer (DNS rebinding): the interface answers only under this computer's
  names (or those listed in `CONSTELLATE_WEB_HOSTS` on a server). Its pages now carry a strict Content Security
  Policy: only the app's own scripts run, and the page talks to nothing but the app.
- A paper's title or abstract can no longer draw a box over the window with oversized math.
- A conference site can no longer make the app reach your own computer or local network through a name that
  points there, nor give papers links that are not web addresses (`javascript:` and the like).
- On Linux and macOS, a new data folder is readable by your account alone, and the engine warns when its `.env`
  (which holds your keys) can be read by other accounts.

## [0.1.0] - 2026-11-26

The first release.

### Added

- **Sources**: accepted papers from the conferences' own lists (NeurIPS, ICML, ICLR, CVPR, ICCV, ECCV, WACV,
  AISTATS, MLSys), other conferences through arXiv comments, and arXiv by date range and category, chosen from
  grouped multi-select menus.
- **Library**: everything downloaded, embedded, described or read is kept by id and never done twice; each step
  can be redone from Settings.
- **Embeddings** with a choice of models (Qwen3-Embedding-0.6B by default), on the GPU when there is one.
- **Communities**: similarity of all papers as max(0, cosine), clustered by spectral clustering, k-nearest
  spectral clustering, k-means or recursive Fiedler bisection, with sub-communities found by further splits.
  A comparison tool (`python -m rp.compare`) measures the methods against each other.
- **Names and descriptions** of communities written by Claude from each community's mean embedding, nearest
  papers and distinguishing terms; a choice of models.
- **Community overviews**: common methods, promising findings and patterns, each point citing its papers.
- **The graph**: one canvas, a glow colour per community, zooming in moves from communities to
  sub-communities, links appear on hover along with the linked papers' titles and tags.
- **The reader**, inside the window: Abstract, the paper's PDF, and its OpenReview page.
- **Acceptance** tiers (oral, spotlight, poster) from the conference lists, shown on papers and communities.
- **What authors declare**: the NeurIPS checklist and the limitations, impact, ethics, reproducibility and
  LLM-usage statements of other venues, read from the PDF, with flags derived from them.
- **Reviewers' ratings**, read from a paper's reviews page when it is opened in the reader.
- **Works without a graphics card**: Settings says whether one was found and what that means for speed, and the
  AI-text check can be switched off; Settings says whether it is on, off or not yet trained, and why.
- **"AI detected"** tag: a statistical estimate from word choice (a classifier trained on abstracts and
  model-written mirrors of them), with its measured error rates shown.
- **Show or dim by kind** on the graph: chips for orals, spotlights, posters, AI detected and not, with counts.
- **Filters**: leave out AI-detected papers, keep only orals or orals and spotlights, leave out papers by
  declaration flag.
- **Cost**: the sidebar shows the model (a menu to change it) and provider in use and what the Claude API has
  cost, in the currency of your region at the European Central Bank's rate.
- **Every number says what it is**: "0.45 cosine similarity", "523 papers", "CAD 2.13", "38k tokens".
- **Runs** can be renamed, deleted and redone.
- **Desktop app** (Electron) with a single build script; tests for the backend and the interface.
- **Windows installer**, built for every release. It needs nothing preinstalled: the first start sets up the
  app's own Python and packages, with the GPU build of PyTorch on machines with an NVIDIA card. The
  uninstaller offers to remove everything the app downloaded and computed as well.

### Security

- The local API answers only this machine and, for anything that changes state, only the app's own interface.
- Pages shown in the reader run in a sandboxed view with its own storage, no permissions, and navigation
  limited to the sites papers are published on.
- PDFs are fetched only from the publishers' hosts, with a size and time limit.

[Unreleased]: https://github.com/Patchoulis/Constellate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Patchoulis/Constellate/releases/tag/v0.1.0
