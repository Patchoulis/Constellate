//! How long the engine's heavy stages take on this computer, on inputs made up here of the size of a large run:
//!
//! ```text
//! cargo run --release --example threads_bench -- [stage ...] [--pdfs FOLDER]
//! CONSTELLATE_THREADS=1 cargo run --release --example threads_bench    # the same on one thread
//! ```
//!
//! The stages are `graph`, `neighbours`, `cluster`, `kmeans`, `citations`, `import` and `pdfs` (all of them when
//! none is named). `pdfs` reads every PDF in `FOLDER` a few times over, for timing only: nothing of the papers is
//! printed but counts. Every input is made from a fixed seed, so two runs time the same work.

use std::path::{Path, PathBuf};
use std::time::Instant;

use constellate::analysis::citations::{LibraryPaper, Linker, Members, ReadPaper, community_citations, network};
use constellate::analysis::graph::cluster::{Limits, Method, Papers, communities};
use constellate::analysis::graph::kmeans::kmeans;
use constellate::analysis::graph::links::{knn_graph, natural_neighbours};
use constellate::analysis::graph::{adjacency_matrix, degrees, make_unit, modularity};
use constellate::text::references::{Confidence, Reference};
use constellate::threads::{self, Progress};
use ndarray::{Array2, Axis};

/// Papers in the made-up run, and numbers per embedding (the default model's).
const PAPERS: usize = 15_000;
const WIDTH: usize = 1_024;
/// Topics the made-up papers are drawn around, so that the graph has communities to find.
const TOPICS: usize = 60;

/// A small seeded generator (xorshift), so that every run makes the same inputs.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() % 2_000_000) as f32 / 1_000_000.0 - 1.0
    }

    fn below(&mut self, count: usize) -> usize {
        (self.next() % count as u64) as usize
    }
}

/// Embeddings drawn around [`TOPICS`] centres.
fn embeddings(papers: usize) -> Array2<f32> {
    let mut random = Seeded(0x2545_F491_4F6C_DD1D);
    let centres = Array2::from_shape_fn((TOPICS, WIDTH), |_| random.unit());
    let mut rows = Array2::from_shape_fn((papers, WIDTH), |_| random.unit() * 1.5);
    for (paper, mut row) in rows.outer_iter_mut().enumerate() {
        row += &centres.row(paper % TOPICS);
    }
    rows
}

fn timed<T>(label: &str, size: &str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = work();
    println!("{label:<44} {size:<28} {:>9.3} s", started.elapsed().as_secs_f64());
    result
}

fn graph_stage() {
    let vectors = embeddings(PAPERS);
    let size = format!("{PAPERS} x {WIDTH}");
    let adjacency = timed("similarity graph (adjacency_matrix)", &size, || adjacency_matrix(vectors.view(), true));
    let Ok(adjacency) = adjacency else { return };
    let halves: Vec<Vec<usize>> = vec![(0..PAPERS / 2).collect(), (PAPERS / 2..PAPERS).collect()];
    timed("degrees", &format!("{PAPERS} x {PAPERS}"), || degrees(adjacency.view()));
    timed("modularity of two halves", &format!("{PAPERS} x {PAPERS}"), || modularity(adjacency.view(), &halves));
}

fn neighbours_stage() {
    let vectors = embeddings(PAPERS);
    let Ok(adjacency) = adjacency_matrix(vectors.view(), true) else { return };
    let size = format!("{PAPERS} x {PAPERS}");
    timed("links drawn (natural_neighbours, 8)", &size, || natural_neighbours(adjacency.view(), 8));
    timed("nearest-neighbour graph (knn_graph, 15)", &size, || knn_graph(adjacency.view(), 15));
    timed("links drawn again (natural_neighbours, 8)", &size, || natural_neighbours(adjacency.view(), 8));
}

fn cluster_stage() {
    let vectors = embeddings(PAPERS);
    let Ok(adjacency) = adjacency_matrix(vectors.view(), true) else { return };
    let mut unit = &vectors - &vectors.mean_axis(Axis(0)).unwrap_or_else(|| ndarray::Array1::zeros(WIDTH));
    make_unit(&mut unit);
    let papers = Papers { adjacency: adjacency.view(), vectors: unit.view() };
    let limits = Limits::default();
    for method in Method::ALL {
        let size = format!("{PAPERS} papers, 20 + 2 levels");
        let label = format!("communities ({method})");
        let made = timed(&label, &size, || communities(method, papers, 20, limits));
        if let Ok(partition) = made {
            let below: usize = partition.communities.iter().map(|community| community.descendants()).sum();
            println!("    {} communities, {below} sub-communities", partition.communities.len());
        }
    }
}

fn kmeans_stage() {
    let vectors = embeddings(PAPERS);
    timed("k-means (20 groups)", &format!("{PAPERS} x {WIDTH}"), || kmeans(vectors.view(), 20, 0));
}

/// A library of `count` papers whose titles are made of words from a small vocabulary, and reference lists that
/// cite them by title, by arXiv id, or not at all.
fn citations_stage() {
    const LIBRARY: usize = 20_000;
    const READ: usize = 20_000;
    const REFERENCES: usize = 40;
    let mut random = Seeded(0x9E37_79B9_7F4A_7C15);
    let vocabulary: Vec<String> = (0..3_000).map(|word| format!("w{word:x}q")).collect();
    let library: Vec<LibraryPaper> = (0..LIBRARY)
        .map(|index| {
            let words: Vec<&str> = (0..8).map(|_| vocabulary[random.below(vocabulary.len())].as_str()).collect();
            LibraryPaper {
                id: format!("p{index}"),
                title: words.join(" "),
                authors: vec!["A. Author".to_owned()],
                year: Some(2020 + (index % 6) as u16),
                venue: None,
                arxiv: (index % 3 == 0).then(|| format!("2401.{index:05}")),
                doi: None,
                url: None,
            }
        })
        .collect();
    let lists: Vec<(String, Vec<Reference>)> = (0..READ)
        .map(|index| {
            let references = (0..REFERENCES)
                .map(|_| {
                    let cited = &library[random.below(LIBRARY)];
                    let kind = random.below(4);
                    Reference {
                        raw: cited.title.clone(),
                        authors: Vec::new(),
                        title: (kind != 0).then(|| cited.title.clone()),
                        year: cited.year,
                        venue: None,
                        arxiv: if kind == 0 { cited.arxiv.clone() } else { None },
                        doi: None,
                        url: None,
                        confidence: Confidence::TitleParsed,
                    }
                })
                .collect();
            (format!("p{index}"), references)
        })
        .collect();
    let linker = timed("citation index (Linker::new)", &format!("{LIBRARY} papers"), || Linker::new(&library));
    let read: Vec<ReadPaper> =
        timed("references linked (Linker::read_all)", &format!("{READ} x {REFERENCES} references"), || {
            linker.read_all(lists)
        });
    let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 27).unwrap_or_default();
    timed("citation network around one paper", &format!("{READ} read papers"), || {
        network("p7", &linker, &read, None, today)
    });
    let run: Vec<String> = (0..READ).map(|index| format!("p{index}")).collect();
    let tree = members_tree(&run, 20, 2);
    timed("community citations", &format!("{READ} papers, 20 + 2 levels"), || community_citations(&read, &run, &tree));
}

/// `count` communities of the run's papers, each cut in two `levels` deep.
fn members_tree(run: &[String], count: usize, levels: usize) -> Vec<Members> {
    fn cut(path: String, papers: Vec<String>, levels: usize) -> Members {
        let children = if levels == 0 || papers.len() < 2 {
            Vec::new()
        } else {
            let half = papers.len() / 2;
            vec![
                cut(format!("{path}.0"), papers[..half].to_vec(), levels - 1),
                cut(format!("{path}.1"), papers[half..].to_vec(), levels - 1),
            ]
        };
        Members { tree_path: path, papers, children }
    }
    (0..count)
        .map(|community| {
            let papers = run.iter().skip(community).step_by(count).cloned().collect();
            cut(format!("c{community}"), papers, levels)
        })
        .collect()
}

/// A files-based library of [`PAPERS`] papers with their vectors, moved into a new database.
fn import_stage() {
    let folder = std::env::temp_dir().join(format!("constellate-bench-{}", std::process::id()));
    let library = folder.join("library");
    if write_files_library(&library).is_err() {
        println!("import: the made-up library could not be written");
        return;
    }
    let imported = timed("import of a files-based library", &format!("{PAPERS} papers + vectors"), || {
        constellate::library::import_files(&library)
    });
    if let Err(error) = imported {
        println!("    import failed: {error}");
    }
    let _ = std::fs::remove_dir_all(&folder);
}

fn write_files_library(folder: &Path) -> std::io::Result<()> {
    let model = folder.join("embeddings").join("Qwen_Qwen3-Embedding-0.6B");
    std::fs::create_dir_all(&model)?;
    let mut lines = String::new();
    let mut index = serde_json::Map::new();
    for paper in 0..PAPERS {
        let id = format!("2601.{paper:05}");
        let record = serde_json::json!({
            "id": id, "title": format!("Paper number {paper}"), "abstract": "An abstract of some length. ".repeat(40),
            "authors": ["A. Author", "B. Author"], "published": "2026-01-02T00:00:00Z", "updated": "",
            "comment": "", "primary_category": "cs.LG", "categories": ["cs.LG"], "venues": ["NeurIPS 2025"],
            "url": null, "pdf_url": null,
        });
        let text = format!("Paper number {paper}\n\n{}", "An abstract of some length. ".repeat(40));
        index.insert(id.clone(), serde_json::json!([paper, constellate::library::text_hash(&text)]));
        lines.push_str(&record.to_string());
        lines.push('\n');
    }
    std::fs::write(folder.join("papers.jsonl"), lines)?;
    std::fs::write(model.join("index.json"), serde_json::Value::Object(index).to_string())?;
    let header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({PAPERS}, {WIDTH}), }}");
    let padded = format!("{header:<117}\n");
    let mut npy = b"\x93NUMPY\x01\x00".to_vec();
    npy.extend((padded.len() as u16).to_le_bytes());
    npy.extend(padded.bytes());
    for value in embeddings(PAPERS).iter() {
        npy.extend(value.to_le_bytes());
    }
    std::fs::write(model.join("vectors.npy"), npy)
}

/// Every PDF in `folder`, read `ROUNDS` times over: text, references, declarations.
fn pdfs_stage(folder: &Path) {
    const ROUNDS: usize = 4;
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect())
        .unwrap_or_default();
    files.retain(|path| path.extension().is_some_and(|extension| extension == "pdf"));
    files.sort();
    let pdfs: Vec<Vec<u8>> = files.iter().filter_map(|path| std::fs::read(path).ok()).collect();
    let batch: Vec<&[u8]> = (0..ROUNDS).flat_map(|_| pdfs.iter().map(Vec::as_slice)).collect();
    let size = format!("{} PDFs ({} files x {ROUNDS})", batch.len(), pdfs.len());
    let texts: Vec<String> = timed("PDF text (pdf_texts)", &size, || {
        let read = constellate::text::pdf::pdf_texts(&batch, &Progress::new());
        read.into_iter().filter_map(|outcome| outcome.done().and_then(Result::ok)).collect()
    });
    let size = format!("{} texts", texts.len());
    timed("reference lists (read_reference_list, each)", &size, || {
        threads::each(&texts, &Progress::new(), |text| {
            constellate::text::references::read_reference_list(text).references.len()
        })
    });
    timed("declarations (read_declarations, each)", &size, || {
        threads::each(&texts, &Progress::new(), |text| {
            constellate::text::declarations::read_declarations(text).flags.len()
        })
    });
}

fn main() {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let pdfs = arguments.iter().position(|argument| argument == "--pdfs").map(|at| {
        let folder = arguments.get(at + 1).cloned().unwrap_or_default();
        arguments.drain(at..(at + 2).min(arguments.len()));
        PathBuf::from(folder)
    });
    let wanted = |stage: &str| arguments.is_empty() || arguments.iter().any(|argument| argument == stage);
    // the pool as the engine builds it, from CONSTELLATE_THREADS
    let cap = threads::cap(std::env::var(threads::VARIABLE).ok().as_deref());
    let Ok(cap) = cap else {
        println!("{} must be a whole number", threads::VARIABLE);
        return;
    };
    if let Err(error) = threads::start(threads::decide(cap)) {
        println!("{error}");
        return;
    }
    println!(
        "{} cores; computing on {} threads",
        std::thread::available_parallelism().map_or(1, usize::from),
        threads::computing()
    );
    if wanted("graph") {
        graph_stage();
    }
    if wanted("neighbours") {
        neighbours_stage();
    }
    if wanted("kmeans") {
        kmeans_stage();
    }
    if wanted("cluster") {
        cluster_stage();
    }
    if wanted("citations") {
        citations_stage();
    }
    if wanted("import") {
        import_stage();
    }
    if let Some(folder) = pdfs.filter(|_| wanted("pdfs")) {
        pdfs_stage(&folder);
    }
}
