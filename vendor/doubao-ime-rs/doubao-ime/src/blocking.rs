//! 同步（阻塞）封装。
//!
//! 内部持有一个多线程 tokio 运行时，把异步 [`crate::Client`] 的方法暴露成阻塞调用，
//! 便于在非 async 代码（以及 FFI）中使用。
//!
//! ```no_run
//! # fn demo() -> doubao_ime::Result<()> {
//! let client = doubao_ime::blocking::Client::new()?;
//! println!("{}", client.organize("嗯那个明天开会")?.content);
//! let text = client.recognize_file("audio.wav", Default::default())?;
//! # let _ = text; Ok(()) }
//! ```

use std::sync::Arc;

use crate::asr::AsrOptions;
use crate::auth::{CredentialOptions, Credentials};
use crate::config::ClientConfig;
use crate::error::Result;
use crate::organize::Organized;

/// 阻塞客户端。`Clone` 廉价，共享同一运行时与连接池。
#[derive(Clone)]
pub struct Client {
    inner: crate::Client,
    rt: Arc<tokio::runtime::Runtime>,
}

impl Client {
    /// 默认配置。
    pub fn new() -> Result<Self> {
        Self::from_config(ClientConfig::default())
    }

    /// 指定配置。
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            inner: crate::Client::from_config(config)?,
            rt: Arc::new(rt),
        })
    }

    /// 获取底层异步客户端。
    pub fn async_client(&self) -> &crate::Client {
        &self.inner
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.rt.block_on(fut)
    }

    /// 文字整理。
    pub fn organize(&self, text: &str) -> Result<Organized> {
        self.block(self.inner.organize(text))
    }

    /// 获取凭据。
    pub fn credentials(&self, opts: CredentialOptions) -> Result<Credentials> {
        self.block(self.inner.credentials(opts))
    }

    /// 识别一段 PCM。
    pub fn recognize_pcm(&self, pcm: &[u8], opts: AsrOptions) -> Result<String> {
        self.block(self.inner.recognize_pcm(pcm, opts))
    }

    /// 识别一个 WAV 文件。
    pub fn recognize_file(
        &self,
        path: impl AsRef<std::path::Path>,
        opts: AsrOptions,
    ) -> Result<String> {
        self.block(self.inner.recognize_file(path, opts))
    }

    /// keyhub 握手。
    pub fn keyhub_handshake(&self, device_id: &str) -> Result<crate::keyhub::Handshake> {
        self.block(self.inner.keyhub_handshake(device_id))
    }

    /// 拉取 TNC 全量配置。
    pub fn fetch_tnc_config(
        &self,
        cronet_version: &str,
        ttnet_version: &str,
    ) -> Result<crate::envelope::TncConfig> {
        self.block(self.inner.fetch_tnc_config(cronet_version, ttnet_version))
    }

    /// 信封加密整理。
    pub fn envelope_organize(
        &self,
        text: &str,
        cfg: &crate::envelope::EnvelopeConfig,
    ) -> Result<crate::envelope::EnvelopeResponse> {
        self.block(self.inner.envelope_organize(text, cfg))
    }
}
