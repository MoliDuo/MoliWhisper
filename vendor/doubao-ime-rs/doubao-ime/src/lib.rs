//! # doubao-ime
//!
//! 豆包输入法**语音识别**（WAV → 文本）与**文字整理**（口语 → 书面语）的异步 Rust 客户端。
//!
//! > ⚠️ **非官方实现。** 基于对豆包输入法（Mac 版）私有接口的逆向分析，并非字节跳动官方 SDK。
//! > 接口、版本参数随时可能变化导致失效；使用前请自行评估是否符合相关服务条款。需要稳定、
//! > 正式授权的能力，请使用火山引擎的官方豆包语音 API。
//!
//! 所有参数在运行时获取或随机生成：无内置密钥、不读写本地状态文件。
//!
//! ## 快速开始
//!
//! ```no_run
//! # async fn demo() -> doubao_ime::Result<()> {
//! use doubao_ime::{Client, AsrOptions};
//!
//! let client = Client::new()?;
//!
//! // 文字整理（无需鉴权）
//! let out = client.organize("嗯那个我想问一下明天几点开会").await?;
//! println!("{}", out.content);
//!
//! // 语音识别（自动获取凭据）
//! let text = client.recognize_file("audio.wav", AsrOptions::default()).await?;
//! println!("{text}");
//! # Ok(()) }
//! ```
//!
//! ## 流式识别 + 实时结果
//!
//! ```no_run
//! # async fn demo(pcm_chunks: Vec<Vec<u8>>) -> doubao_ime::Result<()> {
//! use doubao_ime::{Client, AsrOptions};
//! use doubao_ime::auth::CredentialOptions;
//!
//! let client = Client::new()?;
//! let creds = client.credentials(CredentialOptions::default()).await?;
//! let mut session = client.asr_session(creds, AsrOptions::default()).await?;
//!
//! let mut partials = session.partials();
//! tokio::spawn(async move {
//!     while let Some(text) = partials.recv().await {
//!         println!("… {text}");
//!     }
//! });
//!
//! for pcm in pcm_chunks {
//!     session.send_audio(&pcm).await?;
//! }
//! let final_text = session.finish().await?;
//! # let _ = final_text; Ok(()) }
//! ```
//!
//! 同步（阻塞）封装见 [`blocking`] 模块（`blocking` 特性，默认开启）。

#![warn(missing_docs)]

pub mod auth;
pub mod config;
pub mod error;
pub mod proto;

mod asr;
mod client;
mod envelope;
mod keyhub;
mod organize;
mod util;

#[cfg(feature = "blocking")]
pub mod blocking;

pub use asr::{read_wav_pcm, AsrOptions, AsrSession};
pub use auth::{CredentialOptions, Credentials};
pub use client::{Client, ClientBuilder};
pub use config::{ClientConfig, Endpoints};
pub use envelope::{
    decrypt_envelope, extract_versions_from_lib, EnvelopeConfig, EnvelopeResponse, TncConfig,
    REQUIRED_KEY,
};
pub use error::{Error, ErrorKind, Result};
pub use keyhub::Handshake;
pub use organize::{OrganizeEvent, Organized};
pub use proto::{extract_text, WebSocketRequest, WebSocketResponse};
