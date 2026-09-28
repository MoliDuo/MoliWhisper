//! doubao-ime 命令行工具。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use doubao_ime::auth::CredentialOptions;
use doubao_ime::{AsrOptions, Client, Result};
use futures_util::StreamExt;

#[derive(Parser)]
#[command(name = "doubao-ime", about = "豆包输入法能力客户端（非官方）", version)]
struct Cli {
    /// 输出调试日志
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 语音识别（WAV → 文本）
    Asr {
        /// 16kHz 单声道 16bit WAV
        wav: Option<PathBuf>,
        /// 兼容模式（追加 keyhub 票据等要素）
        #[arg(long)]
        compat: bool,
        /// 仅获取并显示本次运行的凭据要素
        #[arg(long)]
        status: bool,
    },
    /// 文字整理（口语 → 书面语）
    Organize {
        /// 待整理文本
        text: Vec<String>,
        /// SSE 流式输出
        #[arg(long)]
        stream: bool,
    },
    /// keyhub 握手
    Keyhub {
        /// 指定设备 ID（默认随机）
        #[arg(long)]
        device_id: Option<String>,
    },
    /// 拉取 TTNet 全量配置
    Tnc {
        /// 保存响应 JSON
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// cronet 版本参数
        #[arg(long)]
        cronet_version: Option<String>,
        /// ttnet 版本参数
        #[arg(long)]
        ttnet_version: Option<String>,
    },
    /// 信封加密请求
    Envelope {
        /// TNC 配置文件（由 `tnc -o` 生成）
        #[arg(long)]
        config: PathBuf,
        /// 以信封加密方式调用文字整理
        #[arg(long)]
        organize: Option<String>,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let level = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(level)
        .with_writer(std::io::stderr)
        .init();

    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误 [{}]: {e}", e.kind());
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let client = Client::new()?;
    match cli.cmd {
        Cmd::Asr {
            wav,
            compat,
            status,
        } => {
            if status {
                let creds = client
                    .credentials(CredentialOptions {
                        with_ticket: compat,
                    })
                    .await?;
                println!("device_id  : {}", creds.device_id);
                println!("app_key    : {}", creds.app_key);
                println!("sami_token : {} chars", creds.sami_token.len());
                println!(
                    "x-tt-e-k   : {}",
                    creds
                        .ticket
                        .as_deref()
                        .map(|t| format!("{}... ({} chars)", &t[..t.len().min(20)], t.len()))
                        .unwrap_or_else(|| "（未获取）".into())
                );
                return Ok(());
            }
            let Some(wav) = wav else {
                eprintln!("需要提供 WAV 文件（或使用 --status）");
                std::process::exit(2);
            };
            let opts = AsrOptions {
                compat,
                ..Default::default()
            };
            let creds = client
                .credentials(CredentialOptions {
                    with_ticket: compat,
                })
                .await?;
            let mut session = client.asr_session(creds, opts).await?;
            let mut partials = session.partials();
            let printer = tokio::spawn(async move {
                while let Some(t) = partials.recv().await {
                    eprintln!("[partial] {t}");
                }
            });
            let pcm = doubao_ime::read_wav_pcm(&wav)?;
            session.send_audio(&pcm).await?;
            let text = session.finish().await?;
            let _ = printer.await;
            println!(
                "{}",
                if text.is_empty() {
                    "(空)".into()
                } else {
                    text
                }
            );
        }
        Cmd::Organize { text, stream } => {
            let text = text.join(" ");
            if text.is_empty() {
                eprintln!("需要提供待整理文本");
                std::process::exit(2);
            }
            if stream {
                let mut s = std::pin::pin!(client.organize_stream(&text).await?);
                use doubao_ime::OrganizeEvent::*;
                while let Some(ev) = s.next().await {
                    if let Delta(d) = ev? {
                        print!("{d}");
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                }
                println!();
            } else {
                println!("{}", client.organize(&text).await?.content);
            }
        }
        Cmd::Keyhub { device_id } => {
            let did = device_id.unwrap_or_else(gen_device_id);
            let hs = client.keyhub_handshake(&did).await?;
            println!("ticket_exp      : {:?}", hs.ticket_exp);
            println!("ticket_long_exp : {:?}", hs.ticket_long_exp);
            println!("cert_subject    : {:?}", hs.cert_subject);
            println!(
                "session_key     : {}",
                hs.session_key
                    .map(|k| format!("{} bytes", k.len()))
                    .unwrap_or_else(|| "N/A".into())
            );
        }
        Cmd::Tnc {
            out,
            cronet_version,
            ttnet_version,
        } => {
            let c = cronet_version
                .as_deref()
                .unwrap_or(doubao_ime::config::DEFAULT_CRONET_VERSION);
            let t = ttnet_version
                .as_deref()
                .unwrap_or(doubao_ime::config::DEFAULT_TTNET_VERSION);
            let cfg = client.fetch_tnc_config(c, t).await?;
            eprintln!("命中节点 {}", cfg.host);
            let data = cfg.data().and_then(|d| d.as_object());
            println!("键数        : {}", data.map(|o| o.len()).unwrap_or(0));
            if let Some(keys) = data.map(|o| o.keys().cloned().collect::<Vec<_>>()) {
                println!("keys        : {}", keys.join(", "));
            }
            if let Some(path) = out {
                std::fs::write(&path, serde_json::to_vec(&cfg.raw)?)?;
                eprintln!("已保存 -> {}", path.display());
            }
        }
        Cmd::Envelope { config, organize } => {
            let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&config)?)?;
            let cfg = doubao_ime::EnvelopeConfig::from_tnc(&raw)?;
            match organize {
                Some(text) => {
                    let resp = client.envelope_organize(&text, &cfg).await?;
                    eprintln!(
                        "[{}] HTTP {}",
                        if resp.encrypted {
                            "信封加密"
                        } else {
                            "明文"
                        },
                        resp.status
                    );
                    println!("{}", resp.text());
                }
                None => {
                    println!(
                        "bk_k : {}",
                        cfg.bk_k
                            .map(|k| format!("{} bytes", k.len()))
                            .unwrap_or_else(|| "（无）".into())
                    );
                    println!(
                        "bk_t : {}",
                        cfg.bk_t
                            .as_deref()
                            .map(|t| format!("{} chars", t.len()))
                            .unwrap_or_else(|| "（无）".into())
                    );
                    println!("url  : {:?}", cfg.url);
                }
            }
        }
    }
    Ok(())
}

fn gen_device_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:016}", n % 10u128.pow(16))
}
