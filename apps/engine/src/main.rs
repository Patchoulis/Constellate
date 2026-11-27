//! The engine's program: read how to listen (`api::config`), say where it listens in one line on standard output,
//! serve until standard input closes or Ctrl-C.
//!
//! Standard output is for that one line only: the desktop shell reads it over a pipe, and it may carry the token.
//! Whoever starts the engine must not copy it into a log. Everything meant for a person goes to standard error.

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

use constellate::api::Server;
use constellate::api::config::{self, DATA, Environment, Listen};
use constellate::api::routes::{Engine, routes};
use constellate::library::place;
use constellate::model::language;
use constellate::settings::Choices;

/// The choices among the settings. Until the modules that own them are ported (clustering, the acceptance
/// tiers, authors' declarations, the embedding models), they are listed here as the Python
/// backend has them; each moves to its module as that module arrives.
fn choices() -> Choices {
    let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    Choices {
        embedding_models: list(&[
            "Qwen/Qwen3-Embedding-0.6B",
            "BAAI/bge-large-en-v1.5",
            "BAAI/bge-base-en-v1.5",
            "allenai/specter2_base",
            "sentence-transformers/all-mpnet-base-v2",
        ]),
        clustering: list(&["spectral", "spectral-knn", "kmeans", "fiedler"]),
        acceptance: list(&["all", "highlights", "oral"]),
        flags: list(&["llm_declared", "closed_code", "no_limitations", "human_subjects"]),
        describe_models: language::model_ids(),
    }
}

/// Completes when the engine should stop: its standard input closed (the desktop shell keeps it open for as
/// long as it lives, so a shell that closed or crashed leaves no engine behind to block the next start), or
/// Ctrl-C.
async fn told_to_stop() {
    let (closed, input_closed) = tokio::sync::oneshot::channel::<()>();
    // a thread of its own: reading standard input blocks, and a blocked read must not keep the runtime from ending
    std::thread::spawn(move || {
        let mut sink = [0u8; 1024];
        let mut input = io::stdin().lock();
        while matches!(input.read(&mut sink), Ok(read) if read > 0) {}
        let _ = closed.send(());
    });
    let interrupted = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await; // no way to hear Ctrl-C: then only standard input stops it
        }
    };
    tokio::select! {
        _ = input_closed => eprintln!("constellate: standard input closed, stopping"),
        () = interrupted => eprintln!("constellate: interrupted, stopping"),
        () = terminated() => eprintln!("constellate: asked to stop, stopping"),
    }
}

/// SIGTERM, which service managers send to stop a program; never on Windows, which has no such signal.
async fn terminated() {
    #[cfg(unix)]
    if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        signal.recv().await;
        return;
    }
    std::future::pending::<()>().await;
}

/// The warnings about how the engine listens, where nobody starting it can miss them.
fn shout(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    let rule = "!".repeat(100);
    eprintln!("{rule}");
    eprintln!("!! WARNING: THIS ENGINE IS NOT SECURE");
    for warning in warnings {
        eprintln!("!! {warning}");
    }
    eprintln!("{rule}");
}

fn main() -> ExitCode {
    let started = Environment::read(Path::new(".")).and_then(|environment| {
        let listen = Listen::from_environment(&environment)?;
        let threads = config::threads(&environment)?;
        Ok((environment, listen, threads))
    });
    let (environment, listen, threads) = match started {
        Ok(started) => started,
        Err(error) => {
            eprintln!("constellate: not started: {error}");
            return ExitCode::from(2);
        }
    };
    shout(&listen.warnings());
    // the one pool everything computes on, built before anything can compute (src/threads.rs)
    if let Err(error) = constellate::threads::start(threads) {
        eprintln!("constellate: not started: {error}");
        return ExitCode::FAILURE;
    }
    eprintln!("constellate: computing on {threads} threads");
    // never the folder the engine happened to be started from: this system's place for a program's own data
    let data = match place::data_folder(environment.get(DATA).map(str::to_owned)) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("constellate: not started: {error}");
            return ExitCode::from(2);
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("constellate: not started: {error}");
            return ExitCode::FAILURE;
        }
    };
    let served = runtime.block_on(async {
        let mut server = Server::bind(listen, Engine::new(data, choices()), routes())?;
        server.announce_setup_code();
        let mut out = io::stdout().lock();
        writeln!(out, "{}", server.startup_line())?;
        out.flush()?;
        drop(out);
        server.run(told_to_stop()).await
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("constellate: {error}");
            ExitCode::FAILURE
        }
    }
}
