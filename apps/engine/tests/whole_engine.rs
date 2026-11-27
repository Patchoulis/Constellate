//! The engine as the app runs it: the real program, started on a data folder laid out as the earlier (Python)
//! version left it, driven over HTTP with its token the way the interface drives it, and stopped by closing its
//! standard input.
//!
//! Nothing reaches the network or a language model's provider: the program is started with an empty environment
//! (no key of any provider is set) in a folder with no `.env`; the papers are ICLR papers, whose PDFs are
//! published nowhere a program may fetch from; the run is made again from the library's own vectors (the
//! `rerun` path), so the embedding model is not needed; and only routes that estimate are asked of the features
//! that would ask a model.

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use constellate::library::Paper;
use constellate::pipeline::files;
use ndarray::Array2;
use serde_json::{Value, json};

const ENGINE: &str = env!("CARGO_BIN_EXE_constellate");
const RUN: &str = "iclr2026";
const PAPERS: usize = 45;

/// A data folder as the Python version left it: `library/papers.jsonl`, `download_done.json`, the vectors of one
/// model with their index, `usage.json`, `settings.json` beside the library, and one run it made. Made-up papers
/// on three made-up topics.
fn python_data_folder(data: &Path) {
    let library = data.join("library");
    let qwen = library.join("embeddings").join("Qwen_Qwen3-Embedding-0.6B");
    std::fs::create_dir_all(&qwen).unwrap();
    let topics = ["lattice quark gluon hadron", "protein folding enzyme ligand", "policy reward agent rollout"];
    let (mut lines, mut index) = (Vec::new(), serde_json::Map::new());
    let mut vectors = Array2::<f32>::zeros((PAPERS, 4));
    for paper in 0..PAPERS {
        let topic = paper % 3;
        let record = json!({
            "id": format!("iclr-2026-{}", paper + 1),
            "title": format!("Made-up title {paper} on {}", topics[topic]),
            "abstract": format!("Made-up words of {} and more {} words, number {paper}.", topics[topic], topics[topic]),
            "authors": ["A. Author"],
            "published": "2026-01-20T00:00:00Z",
            "updated": "2026-01-20T00:00:00Z",
            "comment": if paper % 9 == 0 { "Accept (Oral)" } else { "Accept (Poster)" },
            "primary_category": "Topic",
            "categories": ["Topic"],
            "url": format!("https://example.org/{paper}"),
            "pdf_url": null,
            "venues": ["ICLR 2026"],
        });
        let kept: Paper = serde_json::from_value(record.clone()).unwrap();
        index.insert(kept.id.clone(), json!([paper, kept.text_hash()]));
        lines.push(record.to_string());
        vectors[[paper, topic]] = 1.0;
        vectors[[paper, 3]] = 0.01 * paper as f32;
    }
    std::fs::write(library.join("papers.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(library.join("download_done.json"), r#"["venue|ICLR 2026|False|official"]"#).unwrap();
    std::fs::write(qwen.join("vectors.npy"), files::npy(vectors.view())).unwrap();
    std::fs::write(qwen.join("index.json"), Value::Object(index).to_string()).unwrap();
    std::fs::write(
        library.join("usage.json"),
        r#"{"since": "2026-02-01", "models": {"claude-opus-5-5": {"calls": 3, "input_tokens": 900, "output_tokens": 300, "usd": 0.04, "unpriced_calls": 0}}}"#,
    )
    .unwrap();
    std::fs::write(
        data.join("settings.json"),
        r#"{"max_communities": 3, "min_community_size": 3, "sub_levels": 1, "clustering": "spectral"}"#,
    )
    .unwrap();
    let run = data.join("runs").join(RUN);
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join(files::RUN),
        r#"{"selection": {"venues": ["ICLR 2026"], "start": null, "end": null, "categories": [], "include_workshops": false}, "settings": {"clustering": "spectral", "max_communities": 3}, "paper_ids": ["iclr-2026-1"]}"#,
    )
    .unwrap();
    std::fs::write(
        run.join(files::COMMUNITIES),
        r#"{"name": "iclr2026", "papers": 1, "model": "Qwen/Qwen3-Embedding-0.6B", "centered": true, "clustering": "spectral", "modularity": 0.3, "ai_flagged": 0, "communities": [{"id": 0, "tree_path": "c1", "size": 1, "name": "A name the Python gave", "description": "d", "keywords": ["quark"], "mix": {}, "mean_inner_similarity": 0.1, "similarity_to_mean": 0.5, "closest_communities": [], "nearest_to_centroid": [], "ai_flagged": 0, "acceptance": {}, "members": [0], "children": []}]}"#,
    )
    .unwrap();
    std::fs::write(run.join(files::GRAPH), r#"{"nodes": [{"id": "iclr-2026-1", "title": "t", "community": 0, "x": 0, "y": 0, "ai": false, "acceptance": "oral"}], "edges": []}"#).unwrap();
}

/// The program, listening, with what its startup line says.
struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    address: String,
    token: String,
    agent: ureq::Agent,
}

impl Engine {
    fn start(data: &Path) -> Engine {
        let mut command = Command::new(ENGINE);
        // no key, no address and no `.env` of the machine running the test can reach the program
        command.env_clear().current_dir(data).env("CONSTELLATE_DATA", data).env("CONSTELLATE_THREADS", "2");
        for kept in ["SYSTEMROOT", "SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR", "PATH"] {
            if let Some(value) = std::env::var_os(kept) {
                command.env(kept, value);
            }
        }
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let said: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("the startup line: {line:?}"));
        assert_eq!((said["scheme"].as_str(), said["address"].as_str()), (Some("http"), Some("127.0.0.1")), "{said}");
        let config = ureq::Agent::config_builder().http_status_as_error(false).proxy(None).build();
        Engine {
            stdin: child.stdin.take(),
            child,
            address: said["listening"].as_str().unwrap().to_owned(),
            token: said["token"].as_str().unwrap().to_owned(),
            agent: config.into(),
        }
    }

    fn ask(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value, Vec<u8>) {
        let url = format!("{}{path}", self.address);
        let bearer = format!("Bearer {}", self.token);
        let response = match (method, body) {
            ("GET", None) => self.agent.get(&url).header("Authorization", &bearer).call(),
            ("DELETE", None) => self.agent.delete(&url).header("Authorization", &bearer).call(),
            (method, body) => {
                let request = match method {
                    "POST" => self.agent.post(&url),
                    "PUT" => self.agent.put(&url),
                    "PATCH" => self.agent.patch(&url),
                    other => panic!("{other}"),
                };
                let text = body.unwrap_or(Value::Null).to_string();
                request.header("Authorization", &bearer).header("Content-Type", "application/json").send(&text)
            }
        };
        let mut response = response.unwrap();
        let status = response.status().as_u16();
        let bytes = response.body_mut().with_config().limit(64 << 20).read_to_vec().unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json, bytes)
    }

    fn get(&self, path: &str) -> Value {
        let (status, json, _) = self.ask("GET", path, None);
        assert_eq!(status, 200, "GET {path}: {json}");
        json
    }

    /// The job's status once it is no longer running.
    fn settled(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = self.get("/api/status");
            if status["state"] != "running" {
                return status;
            }
            assert!(Instant::now() < deadline, "the job did not end: {status}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stop it as the desktop shell does: by closing its standard input. It must end on its own, at once.
    fn stop(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "the engine ended with {status}");
                return;
            }
            assert!(Instant::now() < deadline, "the engine did not stop when its standard input closed");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
    }
}

#[test]
fn the_engine_serves_the_app_from_an_earlier_versions_data_folder_without_the_network_and_stops_with_its_input() {
    let data = tempfile::tempdir().unwrap();
    python_data_folder(data.path());
    let engine = Engine::start(data.path());

    // nothing without the token, and the token in the startup line works
    let refused = engine.agent.get(&format!("{}/api/status", engine.address)).call().unwrap();
    assert_eq!(refused.status().as_u16(), 401);
    let status = engine.get("/api/status");
    assert_eq!((status["state"].clone(), status["mode"].clone()), (json!("idle"), json!("local")), "{status}");
    assert_eq!(status["api_key_set"], false, "no key of the machine reaches the program");

    // settings, the catalogue
    let settings = engine.get("/api/settings");
    assert_eq!(settings["max_communities"], 3, "the earlier version's settings: {settings}");
    let (code, changed, _) = engine.ask("PUT", "/api/settings", Some(json!({"max_links": 6})));
    assert_eq!((code, changed["max_links"].clone()), (200, json!(6)), "{changed}");
    let catalog = engine.get("/api/catalog");
    assert!(catalog["conferences"].as_array().is_some_and(|list| !list.is_empty()), "{catalog}");

    // the library: the earlier version's moved in at the first open, every part of it
    let library = engine.get("/api/library");
    assert_eq!(library["papers"], PAPERS);
    assert_eq!(library["embeddings"]["Qwen/Qwen3-Embedding-0.6B"], PAPERS);
    assert_eq!(
        (library["ai_detector"]["state"].clone(), library["import_problems"].clone()),
        (json!("untrained"), json!([]))
    );
    assert!(library["machine"]["embedding"].is_string(), "{library}");
    let usage = engine.get("/api/usage");
    assert_eq!((usage["calls"].clone(), usage["usd"].clone()), (json!(3), json!(0.04)), "{usage}");
    assert_eq!(engine.get("/api/detector")["state"], "untrained");

    // the run it made: listed, opened, and made again only once confirmed
    let runs = engine.get("/api/runs");
    assert_eq!(runs[0]["name"], RUN, "{runs}");
    assert_eq!(engine.get(&format!("/api/runs/{RUN}"))["communities"][0]["name"], "A name the Python gave");
    let (code, refused, _) = engine.ask("POST", "/api/runs", Some(json!({"rerun": RUN})));
    assert_eq!(code, 409, "{refused}");
    assert_eq!(
        (refused["replaces"]["named"].clone(), refused["replaces"]["communities_will"].clone()),
        (json!(1), json!("change"))
    );
    let (code, started, _) = engine.ask("POST", "/api/runs", Some(json!({"rerun": RUN, "replace": true})));
    assert_eq!((code, started), (200, json!({"ok": true})));
    let done = engine.settled();
    assert_eq!((done["state"].clone(), done["result"].clone()), (json!("done"), json!(RUN)), "{done}");
    let log = done["log"].to_string();
    assert!(
        log.contains("already downloaded") && log.contains("were embedded before"),
        "nothing fetched or embedded: {log}"
    );
    assert!(log.contains("skipped: no cost was accepted"), "nothing asked of a model: {log}");
    let result = engine.get(&format!("/api/runs/{RUN}"));
    assert_eq!(result["papers"], PAPERS);
    let communities = result["communities"].as_array().unwrap();
    assert!(communities.len() >= 2 && communities.iter().all(|community| community["name"].is_null()), "{result}");
    let graph = engine.get(&format!("/api/runs/{RUN}/graph"));
    assert_eq!(graph["nodes"].as_array().unwrap().len(), PAPERS);
    let community = communities[0]["tree_path"].as_str().unwrap().to_owned();

    // a paper: its record, and its PDF, which cannot be had (published nowhere a program may fetch from)
    let paper = engine.get("/api/papers/iclr-2026-1");
    assert_eq!(
        (paper["acceptance"].clone(), paper["ai"].clone(), paper["readable"].clone()),
        (json!("oral"), Value::Null, json!(false))
    );
    let (code, _, _) = engine.ask("GET", "/api/papers/iclr-2026-1/pdf", None);
    assert!((400..500).contains(&code), "a PDF that cannot be had is refused, not fetched: {code}");
    assert_eq!(engine.ask("GET", "/api/papers/nowhere", None).0, 404);

    // every feature's estimate (or the view that carries it): answered, and nothing sent (no key is set, and
    // nothing is started)
    let before = engine.get("/api/usage")["calls"].clone();
    let at = |path: &str| format!("/api/runs/{RUN}{path}");
    let address = json!({"page": 1, "section": "", "paragraph": 0, "start": 0, "end_page": 1, "end": 10});
    let estimates: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "/api/describe/estimate".to_owned(), None),
        ("GET", at("/describe/estimate"), None),
        ("GET", at(&format!("/overview/{community}")), None),
        ("GET", "/api/detector/estimate".to_owned(), None),
        (
            "POST",
            at(&format!("/communities/{community}/stances/estimate")),
            Some(json!({"question": "Does it scale?", "include_unrelated": false, "papers": []})),
        ),
        ("POST", at(&format!("/communities/{community}/stances/proposals/estimate")), Some(json!({"fresh": false}))),
        (
            "POST",
            at("/marked/stances/estimate"),
            Some(
                json!({"question": "Does it scale?", "include_unrelated": false, "papers": ["iclr-2026-1", "iclr-2026-2"]}),
            ),
        ),
        ("POST", at("/extraction/estimate"), Some(json!({"scope": {"community": community}, "part": "sample"}))),
        ("GET", at("/citation-kinds/estimate?scope=sample"), None),
        ("GET", at("/answers/estimate?q=Does%20it%20scale%3F"), None),
        ("GET", "/api/papers/iclr-2026-1/critique".to_owned(), None),
        ("GET", "/api/papers/iclr-2026-1/claims".to_owned(), None),
        ("POST", "/api/papers/iclr-2026-1/explanation/prepare".to_owned(), Some(json!({"address": address}))),
    ];
    for (method, path, body) in estimates {
        let (code, told, _) = engine.ask(method, &path, body);
        assert_eq!(code, 200, "{method} {path}: {told}");
        assert!(told.is_object(), "{method} {path} answers JSON: {told}");
    }
    assert_eq!(engine.get("/api/status")["state"], "done", "no estimate starts a job");
    assert_eq!(engine.get("/api/usage")["calls"], before, "no estimate spends anything");

    engine.stop();
}

#[test]
fn the_engine_stops_when_its_standard_input_closes_even_before_any_request() {
    let data = tempfile::tempdir().unwrap();
    let engine = Engine::start(data.path());
    let started = Instant::now();
    engine.stop();
    assert!(started.elapsed() < Duration::from_secs(10));
}
