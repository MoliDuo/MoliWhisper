//! 流式语音识别会话。
//!
//! [`Client::asr_session`] 建立连接并完成握手，返回 [`AsrSession`]；随后 [`AsrSession::send_audio`]
//! 送入 PCM，[`AsrSession::finish`] 结束并取回文本。识别结果通过 [`AsrSession::partials`]
//! 提供的通道实时获取（可选）。
//!
//! ```text
//! StartTask → StartSession → TaskRequest×N → FinishSession → FinishTask
//! ```

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::auth::Credentials;
use crate::config::*;
use crate::error::{Error, Result};
use crate::proto::{event, WebSocketRequest, WebSocketResponse};
use crate::Client;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// ASR 会话选项。
#[derive(Debug, Clone)]
pub struct AsrOptions {
    /// 兼容模式：查询串附带 keyhub 票据等可选要素（需要凭据含 `ticket`）。
    pub compat: bool,
    /// 每帧音频字节数。
    pub chunk_bytes: usize,
    /// 帧间隔（模拟实时节奏；设 0 尽快发送）。
    pub send_interval: Duration,
    /// 建立连接超时。
    pub open_timeout: Duration,
    /// 结束时等待最终结果的超时。
    pub finish_timeout: Duration,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            compat: false,
            chunk_bytes: ASR_CHUNK_BYTES,
            send_interval: Duration::from_millis(20),
            open_timeout: Duration::from_secs(12),
            finish_timeout: Duration::from_secs(10),
        }
    }
}

/// 一次进行中的识别会话。
pub struct AsrSession {
    ws: Ws,
    creds: Credentials,
    opts: AsrOptions,
    /// 任务 ID（握手后可读）。
    pub task_id: String,
    text: String,
    partial_tx: Option<mpsc::UnboundedSender<String>>,
}

impl Client {
    /// 建立并握手一个识别会话。
    pub async fn asr_session(&self, creds: Credentials, opts: AsrOptions) -> Result<AsrSession> {
        if opts.compat && creds.ticket.is_none() {
            return Err(Error::Config(
                "兼容模式需要 keyhub 票据：credentials(CredentialOptions{with_ticket:true})".into(),
            ));
        }
        let request = self.build_ws_request(&creds, &opts)?;
        let connect = tokio_tungstenite::connect_async(request);
        let (mut ws, _) = tokio::time::timeout(opts.open_timeout, connect)
            .await
            .map_err(|_| Error::Timeout("建立 WebSocket 连接"))??;

        // StartTask → TaskStarted
        send(
            &mut ws,
            self.frame(&creds, event::START_TASK, false, "", None),
        )
        .await?;
        let started = expect_event(&mut ws, event::TASK_STARTED, Duration::from_secs(6)).await?;
        let task_id = started.task_id;

        // StartSession → SessionStarted
        let f = self
            .frame(&creds, event::START_SESSION, true, "{}", None)
            .namespace(ASR_NAMESPACE);
        send(&mut ws, f).await?;
        expect_event(&mut ws, event::SESSION_STARTED, Duration::from_secs(8)).await?;
        tracing::debug!(%task_id, "session started");

        Ok(AsrSession {
            ws,
            creds,
            opts,
            task_id,
            text: String::new(),
            partial_tx: None,
        })
    }

    fn build_ws_request(
        &self,
        creds: &Credentials,
        opts: &AsrOptions,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        let aid = AID.to_string();
        let mut url = url::Url::parse(&self.endpoints().asr_ws)
            .map_err(|e| Error::Config(format!("ASR WS 地址非法: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("app_key", &creds.app_key)
                .append_pair("aid", &aid)
                .append_pair("device_id", &creds.device_id)
                .append_pair("device_platform", DEVICE_PLATFORM);
            if opts.compat {
                q.append_pair("proto-version", "v2")
                    .append_pair("sdk-version", "2")
                    .append_pair("x-tt-e-k", creds.ticket.as_deref().unwrap_or(""))
                    .append_pair("x-tt-env", "");
            }
        }
        let mut request = url.as_str().into_client_request()?;
        let ua = self.config.user_agent.clone().unwrap_or_else(|| {
            if opts.compat {
                UA_MAC.to_owned()
            } else {
                UA.to_owned()
            }
        });
        let h = request.headers_mut();
        h.insert("Proto-Version", HeaderValue::from_static("v2"));
        h.insert("User-Agent", HeaderValue::from_str(&ua).unwrap());
        h.insert(
            "X-Api-Resource-Id",
            HeaderValue::from_static(ASR_RESOURCE_ID),
        );
        if opts.compat {
            h.insert("sdk-version", HeaderValue::from_static("2"));
        }
        Ok(request)
    }

    fn frame(
        &self,
        creds: &Credentials,
        event: &str,
        with_token: bool,
        payload: &str,
        audio: Option<&[u8]>,
    ) -> WebSocketRequest {
        let mut f = WebSocketRequest::new(event).appkey(&creds.app_key);
        if with_token {
            f = f.token(&creds.sami_token);
        }
        if !payload.is_empty() {
            f = f.payload(payload);
        }
        if let Some(pcm) = audio {
            f = f.audio(pcm);
        }
        f
    }
}

impl std::fmt::Debug for AsrSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsrSession")
            .field("task_id", &self.task_id)
            .field("text_len", &self.text.len())
            .field("compat", &self.opts.compat)
            .finish()
    }
}

impl AsrSession {
    /// 已识别的累积文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 订阅实时结果通道；每次累积全文更新推送一次。须在 [`send_audio`](Self::send_audio) 前调用。
    pub fn partials(&mut self) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.partial_tx = Some(tx);
        rx
    }

    /// 发送一段 PCM（16kHz 单声道 s16le），发送过程中顺带接收增量结果。可多次调用。
    pub async fn send_audio(&mut self, pcm: &[u8]) -> Result<()> {
        let chunk = self.opts.chunk_bytes.max(1);
        for part in pcm.chunks(chunk) {
            let frame = WebSocketRequest::new(event::TASK_REQUEST)
                .appkey(&self.creds.app_key)
                .payload("{}")
                .audio(part);
            send(&mut self.ws, frame).await?;
            self.drain_ready()?;
            if !self.opts.send_interval.is_zero() {
                tokio::time::sleep(self.opts.send_interval).await;
            }
        }
        Ok(())
    }

    /// 非阻塞地取走当前已到达的结果帧。
    fn drain_ready(&mut self) -> Result<()> {
        while let Some(msg) = self.ws.next().now_or_never().flatten() {
            let msg = msg?;
            if let Some(done) = self.handle(msg)? {
                if done {
                    break;
                }
            }
        }
        Ok(())
    }

    /// 结束会话，等待最终结果并返回识别文本。消费 `self`。
    pub async fn finish(mut self) -> Result<String> {
        let finish = WebSocketRequest::new(event::FINISH_SESSION)
            .appkey(&self.creds.app_key)
            .token(&self.creds.sami_token);
        send(&mut self.ws, finish).await?;

        let deadline = tokio::time::sleep(self.opts.finish_timeout);
        tokio::pin!(deadline);
        let mut finished = false;
        loop {
            tokio::select! {
                biased;
                _ = &mut deadline => {
                    tracing::warn!("等待 SessionFinished 超时，返回当前结果");
                    break;
                }
                msg = self.ws.next() => match msg {
                    Some(msg) => if self.handle(msg?)?.unwrap_or(false) { finished = true; break; },
                    None => break,
                }
            }
        }
        let _ = finished;

        let finish_task = WebSocketRequest::new(event::FINISH_TASK)
            .appkey(&self.creds.app_key)
            .token(&self.creds.sami_token);
        let _ = send(&mut self.ws, finish_task).await;
        let _ = self.ws.close(None).await;
        Ok(self.text)
    }

    /// 处理一帧；返回 `Some(true)` 表示会话结束。
    fn handle(&mut self, msg: Message) -> Result<Option<bool>> {
        let data = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => return Ok(Some(true)),
            _ => return Ok(None),
        };
        let resp = WebSocketResponse::from_bytes(&data)?;
        if resp.is_failure() {
            return Err(Error::Asr(format!(
                "{}: [{}] {}",
                resp.event, resp.status_code, resp.status_text
            )));
        }
        let finished = resp.event == event::SESSION_FINISHED;
        if let Some(text) = resp.text() {
            if text != self.text {
                self.text = text.clone();
                if let Some(tx) = &self.partial_tx {
                    let _ = tx.send(text);
                }
            }
        }
        Ok(Some(finished))
    }
}

use futures_util::FutureExt;

async fn send(ws: &mut Ws, frame: WebSocketRequest) -> Result<()> {
    ws.send(Message::Binary(frame.to_bytes())).await?;
    Ok(())
}

async fn expect_event(ws: &mut Ws, want: &str, timeout: Duration) -> Result<WebSocketResponse> {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .map_err(|_| Error::Timeout("等待服务端事件"))?
        .ok_or_else(|| Error::Asr("连接在握手期间关闭".into()))??;
    let data = match msg {
        Message::Binary(b) => b,
        other => return Err(Error::Asr(format!("握手期间收到非二进制帧: {other:?}"))),
    };
    let resp = WebSocketResponse::from_bytes(&data)?;
    if resp.event != want {
        return Err(Error::Asr(format!(
            "期望 {want}，收到 {}: [{}] {}",
            if resp.event.is_empty() {
                "(空)"
            } else {
                &resp.event
            },
            resp.status_code,
            resp.status_text
        )));
    }
    Ok(resp)
}

/// 读取 WAV 文件为 PCM，并校验格式（16kHz / 单声道 / 16bit）。
pub fn read_wav_pcm(path: impl AsRef<std::path::Path>) -> Result<Vec<u8>> {
    let reader = hound::WavReader::open(path).map_err(hound_err)?;
    let spec = reader.spec();
    if spec.sample_rate != ASR_SAMPLE_RATE || spec.channels != 1 || spec.bits_per_sample != 16 {
        return Err(Error::InvalidAudio(format!(
            "需要 16kHz 单声道 16bit，实际 {}Hz / {} 声道 / {}bit",
            spec.sample_rate, spec.channels, spec.bits_per_sample
        )));
    }
    let mut pcm = Vec::with_capacity(reader.len() as usize * 2);
    for s in reader.into_samples::<i16>() {
        pcm.extend_from_slice(&s.map_err(hound_err)?.to_le_bytes());
    }
    Ok(pcm)
}

fn hound_err(e: hound::Error) -> Error {
    match e {
        hound::Error::IoError(io) => Error::Io(io),
        other => Error::InvalidAudio(other.to_string()),
    }
}

/// 便捷函数：识别一段 PCM（自动获取凭据）。
impl Client {
    /// 一次性识别整段 PCM。
    pub async fn recognize_pcm(&self, pcm: &[u8], opts: AsrOptions) -> Result<String> {
        let creds = self
            .credentials(crate::auth::CredentialOptions {
                with_ticket: opts.compat,
            })
            .await?;
        let mut session = self.asr_session(creds, opts).await?;
        session.send_audio(pcm).await?;
        session.finish().await
    }

    /// 一次性识别一个 WAV 文件。
    pub async fn recognize_file(
        &self,
        path: impl AsRef<std::path::Path>,
        opts: AsrOptions,
    ) -> Result<String> {
        let pcm = read_wav_pcm(path)?;
        self.recognize_pcm(&pcm, opts).await
    }
}
