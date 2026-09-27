//! Probe for the live Doubao ASR endpoint.
//!
//! Streams a 16 kHz mono WAV at real-time pace, stops with the chosen strategy
//! and reports how complete the final text is and how long it took to arrive.
//!
//! ```text
//! cargo run -p moli-core --example asr_probe -- \
//!     --wav crates/moli-core/fixtures/zh_short.wav --strategy finish --repeat 20
//! cargo run -p moli-core --example asr_probe -- --verify
//! ```
//!
//! By default it uses the credentials the app saved (the keychain may ask for
//! access). `--creds file.json` or `MOLI_CREDS` points at a plain JSON file
//! `{device_id, web_id, cookies: {name: value}}` instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use moli_core::asr::params::Overrides;
use moli_core::asr::{AsrEvent, ConnectOptions, ServerMsg, connect, verify};
use moli_core::creds::Credentials;
use moli_core::store::CredStore;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Strategy {
    /// Send `{"event":"finish"}` and wait for the server's `finish`.
    Finish,
    /// Pad 200 ms of silence, then wait until results go quiet for 250 ms (max 1.5 s).
    Silence,
    /// Silence padding, then the finish frame.
    Both,
    /// Close right away.
    Close,
    /// Send nothing more; watch what the server does for `--wait-ms`.
    Nothing,
}

struct Args {
    wav: PathBuf,
    creds: Option<PathBuf>,
    strategy: Strategy,
    repeat: usize,
    chunk: usize,
    fast: bool,
    trim: bool,
    idle_ms: u64,
    wait_ms: u64,
    gap_ms: u64,
    expect: Option<String>,
    origin: bool,
    user_agent: bool,
    overrides: Overrides,
    verbose: bool,
    verify: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args {
        wav: PathBuf::new(),
        creds: std::env::var_os("MOLI_CREDS").map(PathBuf::from),
        strategy: Strategy::Finish,
        repeat: 1,
        chunk: 4096,
        fast: false,
        trim: true,
        idle_ms: 0,
        wait_ms: 3000,
        gap_ms: 500,
        expect: None,
        origin: true,
        user_agent: true,
        overrides: Overrides::new(),
        verbose: false,
        verify: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--wav" => a.wav = val()?.into(),
            "--creds" => a.creds = Some(val()?.into()),
            "--strategy" => {
                a.strategy = match val()?.as_str() {
                    "finish" => Strategy::Finish,
                    "silence" => Strategy::Silence,
                    "both" => Strategy::Both,
                    "close" => Strategy::Close,
                    "nothing" => Strategy::Nothing,
                    s => bail!("unknown strategy {s}"),
                }
            }
            "--repeat" => a.repeat = val()?.parse()?,
            "--chunk" => a.chunk = val()?.parse()?,
            "--fast" => a.fast = true,
            "--no-trim" => a.trim = false,
            "--idle-ms" => a.idle_ms = val()?.parse()?,
            "--wait-ms" => a.wait_ms = val()?.parse()?,
            "--gap-ms" => a.gap_ms = val()?.parse()?,
            "--expect" => a.expect = Some(val()?),
            "--no-origin" => a.origin = false,
            "--no-ua" => a.user_agent = false,
            "--legacy" => {
                // The parameter set used before the 3.38.5 web client.
                a.overrides
                    .insert("pc_version".into(), Some("3.12.3".into()));
                for k in [
                    "doubao_pc_version",
                    "doubao_device_platform",
                    "web_platform",
                ] {
                    a.overrides.insert(k.into(), None);
                }
            }
            "--set" => {
                let kv = val()?;
                let (k, v) = kv.split_once('=').context("--set wants key=value")?;
                a.overrides.insert(k.into(), Some(v.into()));
            }
            "--unset" => {
                a.overrides.insert(val()?, None);
            }
            "-v" | "--verbose" => a.verbose = true,
            "--verify" => a.verify = true,
            _ => bail!("unknown flag {flag}"),
        }
    }
    if a.wav.as_os_str().is_empty() && !a.verify {
        bail!("--wav is required");
    }
    if a.expect.is_none() {
        a.expect = std::fs::read_to_string(a.wav.with_extension("txt")).ok();
    }
    Ok(a)
}

fn load_creds(path: Option<&Path>) -> anyhow::Result<Credentials> {
    let (creds, path) = match path {
        Some(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
            let creds: Credentials = serde_json::from_str(&text).context("parse credentials")?;
            (creds, p.to_path_buf())
        }
        None => {
            let store = CredStore::open_default()?;
            let stored = store
                .load()?
                .with_context(|| format!("not logged in ({} missing)", store.path().display()))?;
            if let Some(at) = stored.rejected_at {
                eprintln!("warning: the app marked this session rejected at unix {at}");
            }
            (stored.credentials, store.path().to_path_buf())
        }
    };
    if !creds.has_session() {
        eprintln!(
            "warning: credentials in {} lack sessionid/sid_guard/device_id/web_id",
            path.display()
        );
    }
    Ok(creds)
}

fn load_pcm(path: &Path, trim: bool) -> anyhow::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 || spec.bits_per_sample != 16 {
        bail!(
            "{} must be 16 kHz mono 16-bit, got {spec:?}",
            path.display()
        );
    }
    let mut samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
    if trim {
        // Stop right on the last syllable: drop trailing samples below ~-40 dBFS.
        let end = samples
            .iter()
            .rposition(|s| s.unsigned_abs() > 330)
            .map_or(0, |i| i + 1);
        samples.truncate(end);
    }
    Ok(samples)
}

fn to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Keeps only letters and digits, so punctuation choices don't count as misses.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

#[derive(Debug, Default)]
struct Outcome {
    connect_ms: u128,
    text: String,
    /// Stop → last change of the text.
    last_result_ms: Option<u128>,
    /// Stop → server `finish`.
    finish_ms: Option<u128>,
    close: Option<String>,
    errors: Vec<String>,
    results: usize,
}

async fn run_once(args: &Args, creds: &Credentials, pcm: &[i16]) -> anyhow::Result<Outcome> {
    let mut opts = ConnectOptions::new(creds, &args.overrides);
    if !args.origin {
        opts.origin = None;
    }
    if !args.user_agent {
        opts.user_agent = None;
    }
    let mut out = Outcome::default();
    let (mut sink, mut stream, handshake) = match connect(&opts).await {
        Ok(v) => v,
        Err(e) => {
            out.errors.push(format!("connect: {e}"));
            return Ok(out);
        }
    };
    out.connect_ms = handshake.elapsed.as_millis();
    if args.verbose && !handshake.set_cookies.is_empty() {
        let names: Vec<_> = handshake
            .set_cookies
            .iter()
            .filter_map(|c| c.split('=').next())
            .collect();
        println!("    handshake set-cookie: {names:?}");
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Some(ev) = stream.next().await {
            if tx.send((Instant::now(), ev)).is_err() {
                break;
            }
        }
    });

    let t0 = Instant::now();
    let log = |at: Instant, ev: &AsrEvent| {
        if args.verbose {
            println!("    {:>6}ms {ev:?}", at.duration_since(t0).as_millis());
        }
    };
    let apply = |out: &mut Outcome, at: Instant, ev: AsrEvent, stop: Option<Instant>| -> bool {
        log(at, &ev);
        let since_stop = stop.map(|s| at.saturating_duration_since(s).as_millis());
        match ev {
            AsrEvent::Server(ServerMsg::Result { text }) => {
                out.results += 1;
                if text != out.text {
                    out.text = text;
                    out.last_result_ms = since_stop;
                }
            }
            AsrEvent::Server(ServerMsg::Finish) => {
                out.finish_ms = since_stop.or(Some(0));
                if stop.is_none() {
                    out.errors.push("server finished before stop".into());
                }
            }
            AsrEvent::Server(ServerMsg::Error { code, message }) => {
                out.errors.push(format!("server error {code}: {message}"))
            }
            AsrEvent::Server(ServerMsg::Unknown { event }) => {
                out.errors.push(format!("unknown event {event}"))
            }
            AsrEvent::Garbage(s) => out.errors.push(format!("garbage: {s}")),
            AsrEvent::Closed {
                code,
                reason,
                received_any,
            } => {
                out.close = Some(format!("{code:?} {reason:?} received_any={received_any}"));
                return true;
            }
            AsrEvent::Failed(e) => {
                out.errors.push(format!("transport: {e}"));
                return true;
            }
        }
        false
    };

    let mut closed = false;
    if args.idle_ms > 0 {
        let until = Instant::now() + Duration::from_millis(args.idle_ms);
        while let Ok(Some((at, ev))) = tokio::time::timeout_at(until.into(), rx.recv()).await {
            closed |= apply(&mut out, at, ev, None);
        }
    }

    let samples_per_chunk = (args.chunk / 2).max(1);
    let chunk_dur = Duration::from_secs_f64(samples_per_chunk as f64 / 16_000.0);
    let send_start = Instant::now();
    for (i, chunk) in pcm.chunks(samples_per_chunk).enumerate() {
        if closed {
            break;
        }
        if !args.fast {
            // Real-time pacing: a chunk goes out once it would have been captured.
            tokio::time::sleep_until((send_start + chunk_dur * (i as u32 + 1)).into()).await;
        }
        if let Err(e) = sink.audio(to_bytes(chunk)).await {
            out.errors.push(e.to_string());
            closed = true;
        }
        while let Ok((at, ev)) = rx.try_recv() {
            closed |= apply(&mut out, at, ev, None);
        }
    }

    let stop = Instant::now();
    if !closed {
        let silence = |ms: u64| to_bytes(&vec![0i16; (16 * ms) as usize]);
        let result = match args.strategy {
            Strategy::Finish => sink.finish().await,
            Strategy::Silence => sink.audio(silence(200)).await,
            Strategy::Both => match sink.audio(silence(200)).await {
                Ok(()) => sink.finish().await,
                e => e,
            },
            Strategy::Close => sink.close().await,
            Strategy::Nothing => Ok(()),
        };
        if let Err(e) = result {
            out.errors.push(format!("stop: {e}"));
        }

        let hard_deadline = stop
            + Duration::from_millis(match args.strategy {
                Strategy::Silence => 1500,
                _ => args.wait_ms,
            });
        let mut last_change = stop;
        loop {
            let deadline = match args.strategy {
                Strategy::Silence => hard_deadline.min(last_change + Duration::from_millis(250)),
                _ => hard_deadline,
            };
            match tokio::time::timeout_at(deadline.into(), rx.recv()).await {
                Ok(Some((at, ev))) => {
                    let before = out.text.clone();
                    let finished = matches!(ev, AsrEvent::Server(ServerMsg::Finish));
                    if apply(&mut out, at, ev, Some(stop)) {
                        break;
                    }
                    if out.text != before {
                        last_change = at;
                    }
                    if finished && args.strategy != Strategy::Nothing {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        let _ = sink.close().await;
        // Pick up the close frame so it shows in the log.
        let drain_until = Instant::now() + Duration::from_millis(500);
        while let Ok(Some((at, ev))) = tokio::time::timeout_at(drain_until.into(), rx.recv()).await
        {
            if apply(&mut out, at, ev, Some(stop)) {
                break;
            }
        }
    }
    reader.abort();
    Ok(out)
}

fn percentile(sorted: &[u128], p: f64) -> Option<u128> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    Some(sorted[idx])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let args = parse_args()?;
    let creds = load_creds(args.creds.as_deref())?;
    if args.verify {
        for i in 0..args.repeat {
            let t = Instant::now();
            let verdict = verify(&ConnectOptions::new(&creds, &args.overrides)).await;
            println!(
                "#{:<2} {verdict:?} in {} ms",
                i + 1,
                t.elapsed().as_millis()
            );
        }
        return Ok(());
    }
    let pcm = load_pcm(&args.wav, args.trim)?;
    let expect = args.expect.as_deref().map(normalize);
    println!(
        "wav {} ({:.2}s{}), strategy {:?}, chunk {} B, overrides {:?}, origin {}, ua {}",
        args.wav.display(),
        pcm.len() as f64 / 16_000.0,
        if args.trim {
            ", trailing silence trimmed"
        } else {
            ""
        },
        args.strategy,
        args.chunk,
        args.overrides,
        args.origin,
        args.user_agent,
    );
    if let Some(exp) = creds.expires_at() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        println!(
            "session expires in {:.1} days",
            (exp as f64 - now as f64) / 86400.0
        );
    }

    let mut complete = 0;
    let mut latencies = Vec::new();
    let mut finish_latencies = Vec::new();
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    for i in 0..args.repeat {
        let o = run_once(&args, &creds, &pcm).await?;
        let ok = expect.as_ref().is_some_and(|e| normalize(&o.text) == *e);
        complete += usize::from(ok);
        latencies.extend(o.last_result_ms);
        finish_latencies.extend(o.finish_ms);
        for e in &o.errors {
            *failures.entry(e.clone()).or_default() += 1;
        }
        println!(
            "#{:<2} {} connect {}ms | last result +{} | finish +{} | {} results | close {} | {:?}{}",
            i + 1,
            if ok { "OK  " } else { "MISS" },
            o.connect_ms,
            o.last_result_ms.map_or("-".into(), |v| format!("{v}ms")),
            o.finish_ms.map_or("-".into(), |v| format!("{v}ms")),
            o.results,
            o.close.as_deref().unwrap_or("-"),
            o.text,
            if o.errors.is_empty() {
                String::new()
            } else {
                format!(" errors {:?}", o.errors)
            },
        );
        if i + 1 < args.repeat {
            tokio::time::sleep(Duration::from_millis(args.gap_ms)).await;
        }
    }

    latencies.sort_unstable();
    finish_latencies.sort_unstable();
    println!(
        "\ncomplete {complete}/{} | stop→last result p50 {:?} p95 {:?} | stop→finish p50 {:?} p95 {:?} (n={})",
        args.repeat,
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
        percentile(&finish_latencies, 0.5),
        percentile(&finish_latencies, 0.95),
        finish_latencies.len(),
    );
    if !failures.is_empty() {
        println!("errors: {failures:?}");
    }
    Ok(())
}
