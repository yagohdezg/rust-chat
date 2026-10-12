mod common;

use std::time::{Duration, Instant};

use common::{
    collect_sse, delta_text, events_named, Backend, Script, Session, TestEnvBuilder, Turn,
};

const STREAM_ENV: &str = "RUSTCHAT_LOAD_STREAMS";
const DELAY_ENV: &str = "RUSTCHAT_LOAD_DELAY_MS";
const SAMPLE_ENV: &str = "RUSTCHAT_LOAD_SAMPLE_MS";

const DEFAULT_DELAY_MS: u64 = 120;
const DEFAULT_SAMPLE_MS: u64 = 50;
const TURNS_PER_STREAM: usize = 8;
const CHUNK: &str = "the quick brown fox jumps over the lazy dog. ";

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn reply_text() -> String {
    CHUNK.repeat(TURNS_PER_STREAM)
}

fn script(delay_ms: u64) -> Script {
    let mut turns = Vec::with_capacity(TURNS_PER_STREAM + 1);
    for _ in 0..TURNS_PER_STREAM {
        turns.push(Turn::Text(CHUNK.to_string()));
    }
    turns.push(Turn::Finish);
    Script {
        model: "fake-model".into(),
        responses: vec![turns],
        delay_ms,
    }
}

fn vm_rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[tokio::test]
#[ignore = "load/soak: run with `just e2e-load` (cargo test -p chat-server --test e2e_load -- --ignored --nocapture)"]
async fn concurrent_sse_streams_stay_within_budget() {
    let backend = if common::postgres_available() {
        Backend::Postgres
    } else {
        Backend::Sqlite
    };
    let default_streams = if backend == Backend::Postgres { 50 } else { 4 };
    let streams = env_usize(STREAM_ENV, default_streams);
    let delay_ms = env_u64(DELAY_ENV, DEFAULT_DELAY_MS);
    let sample_ms = env_u64(SAMPLE_ENV, DEFAULT_SAMPLE_MS);

    let env = TestEnvBuilder::new()
        .backend(backend)
        .script(script(delay_ms))
        .start()
        .await
        .unwrap();

    let session = Session::register(&env.base_url, "load@example.com", "password123")
        .await
        .unwrap();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();

    let mut conversations = Vec::with_capacity(streams);
    for _ in 0..streams {
        conversations.push(session.create_conversation("load").await.unwrap());
    }

    let pid = env.server_pid();
    let baseline = vm_rss_bytes(pid).expect("read VmRSS baseline");
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(baseline));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let peak = peak.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(rss) = vm_rss_bytes(pid) {
                    peak.fetch_max(rss, std::sync::atomic::Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_millis(sample_ms)).await;
            }
        });
    }

    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for conversation_id in conversations {
        let session = session.clone();
        let expected = reply_text();
        tasks.spawn(async move {
            let response = session
                .chat(conversation_id, "hello", None, Some(provider_id))
                .await
                .expect("open chat SSE stream");
            assert!(
                response.status().is_success(),
                "chat stream status {}",
                response.status()
            );
            let events = collect_sse(response).await;
            let deltas = delta_text(&events);
            let done = events_named(&events, "done").len();
            let errors = events_named(&events, "error").len();
            (deltas, done, errors, expected)
        });
    }

    let mut failures = Vec::new();
    let mut completed = 0usize;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((deltas, done, errors, expected)) => {
                completed += 1;
                if done != 1 || errors != 0 || deltas != expected {
                    failures.push(format!(
                        "done={done} errors={errors} bytes={} expected={}",
                        deltas.len(),
                        expected.len()
                    ));
                }
            }
            Err(err) => failures.push(format!("task panicked: {err}")),
        }
    }

    let elapsed = started.elapsed();
    let peak_bytes = peak.load(std::sync::atomic::Ordering::Relaxed);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let bytes_per_stream = peak_bytes / streams as u64;
    let reply_bytes = reply_text().len();

    eprintln!(
        "load: backend={backend:?} streams={streams} completed={completed} duration={:.2}s delay_ms={delay_ms}",
        elapsed.as_secs_f64()
    );
    eprintln!(
        "load: reply_bytes={reply_bytes} baseline_rss={:.2} MiB peak_rss={} kB ({:.2} MiB) bytes_per_stream={bytes_per_stream}",
        baseline as f64 / (1024.0 * 1024.0),
        peak_bytes / 1024,
        peak_bytes as f64 / (1024.0 * 1024.0),
    );

    assert!(failures.is_empty(), "stream failures: {failures:#?}");
    assert_eq!(completed, streams, "every stream must finish");
    assert_eq!(env.provider.requests().len(), streams);
}
