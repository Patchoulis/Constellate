# Changelog

What changed in Constellate, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The paper reader has a Weaknesses section: the reviewers' own weaknesses, questions and limitations point by point, with each reviewer's rating and confidence and any reply of the authors, each point opening on the reviewer's words; and, on request with its cost estimated first, the model's reading of the paper (where each point applies, what the paper says about it, and weaknesses the reviewers did not raise), every statement linked to the paper's own words, kept apart and never a score or verdict.

- Opening a paper's Reviews tab now keeps the whole discussion (each review by section, rebuttals, the meta-review and the decision), shows the paper's mean, range and confidence-weighted mean on its venue's own scale, and adds a Reviews section to each community: how many of its papers have reviews read, ratings per conference and year, the papers reviewers disagreed on most, and what reviewers most often praise and criticise, each linked to the reviewer's words.

- The paper reader has a Text tab showing the paper's text section by section, and citations open it at the exact place they quote, with the words highlighted; quotes Claude gives as evidence are only shown once they have been found in the paper they are attributed to.

- The runs sidebar, the communities panel, the paper reader, the overview and the activity log can be resized by dragging their edge (or with the arrow keys once the edge is focused); double-click an edge to put it back, and the sizes are remembered.

- `npm run setup` (or `make setup`) gets a fresh clone ready to work on in one command.

- A Citations tab in the reader shows a paper's citation network: the works it cites and the papers that cite it,
  laid out by year with the links between them, and as lists you can filter and sort. It is built from the
  reference lists of the papers Constellate has read, so "cited by" counts only those papers. Communities can be
  sorted by how often they are cited and by how many works they cite.

- The engine uses any maker's graphics card: through DirectML on Windows and through Vulkan (WebGPU) on Linux, with
  the processor as the fallback. It is built and tested on Linux as well as Windows.

- A Recenter button on the graph brings back what you were looking at: the open paper, else the focused community,
  else the whole graph. The graph glides there instead of jumping, as it does when a community is focused or a
  paper opened; panels slide in and labels fade in. None of it moves for someone who asked their system for less
  motion.
- Mark papers to keep them in view: right-click a paper on the graph (or use the star in the reader and in a
  community's list) and it gets a yellow ring with its closest papers always linked and named; the Marked tab lists
  a run's marked papers, and marks are remembered for each run. While one community is in focus, only the marked
  papers inside it are shown.
- The engine has the model prices and the exchange rates built in, with the day each rate is from: once it
  replaces the backend, the app no longer asks the internet for a rate. A weekly check compares the rates with the
  European Central Bank's and, when one has moved by more than 1%, prepares a new version with the new rates.
- The engine keeps the library in one small database on your computer instead of many loose files, so an
  interrupted write can no longer leave it half-saved, and an existing library is moved in without losing anything.
  A conference whose site did not answer is now fetched again later instead of being counted as downloaded, and
  papers whose PDF could not be had or read are remembered as such.

### Changed

- The engine computes on every core: the similarity graph, the communities, nearest neighbours and citation figures
  are worked out several times faster with the same results, PDFs are read side by side, and downloads run a few at a
  time within each site's limits (arXiv still one request every three seconds). `CONSTELLATE_THREADS` caps the
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

- Marked papers and panel sizes are kept between launches: the interface is served on the same port each time.
- The new engine now serves the reader's Text tab and finds the words a citation quotes; each paper's PDF is read
  once and kept, and is read again only when the PDF or the way papers are read changes.
- A Claude API key (and address) set in a `.env` file beside the new engine is now used, like its other settings;
  the key is never shown, logged or passed on to another program.

### Security

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
