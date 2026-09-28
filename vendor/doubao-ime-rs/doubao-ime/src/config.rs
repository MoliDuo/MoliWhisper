//! 常量、接口地址与客户端配置。

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 豆包输入法应用 ID。
pub const AID: u32 = 685_343;
/// 设备平台标识。
pub const DEVICE_PLATFORM: &str = "mac";
/// 默认 User-Agent。
pub const UA: &str = "DoubaoIme/1.0.1";
/// 兼容模式使用的完整 User-Agent。
pub const UA_MAC: &str = "DoubaoIme/1.0.1 (Macintosh; Intel Mac OS X 26_0)";

/// ASR 命名空间。
pub const ASR_NAMESPACE: &str = "ASR";
/// ASR 资源 ID（握手头 `X-Api-Resource-Id`）。
pub const ASR_RESOURCE_ID: &str = "original.sami.ASR";
/// ASR 要求的采样率。
pub const ASR_SAMPLE_RATE: u32 = 16_000;
/// 默认每帧音频字节数（200ms @ 16kHz s16le 单声道）。
pub const ASR_CHUNK_BYTES: usize = 6_400;
/// 服务端成功状态码。
pub const STATUS_OK: i64 = 20_000_000;

/// "文字整理" 场景的服务端枚举值。
pub const ORGANIZE_SCENE: i32 = 6;

/// 默认 cronet 版本参数（随客户端版本更新）。
pub const DEFAULT_CRONET_VERSION: &str = "e8646bd5_2026-09-18";
/// 默认 ttnet 版本参数（随客户端版本更新）。
pub const DEFAULT_TTNET_VERSION: &str = "4.2.243.39-doubao";

/// 所有服务端地址。可整体或逐项覆盖（例如指向测试服务器或代理）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Endpoints {
    /// TTKitchen 配置（下发 app_key）。
    pub ttkitchen: String,
    /// SAMI token 换发接口。
    pub sami_config: String,
    /// ASR WebSocket 地址。
    pub asr_ws: String,
    /// 文字整理接口。
    pub organize: String,
    /// keyhub 握手接口。
    pub keyhub: String,
    /// TNC 配置接口（按顺序尝试，完整 URL 不含查询串）。
    pub tnc: Vec<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            ttkitchen: "https://is.snssdk.com/service/settings/v3/".into(),
            sami_config: "https://ime.oceancloudapi.com/api/v1/user/get_config".into(),
            asr_ws: "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws".into(),
            organize: "https://ime.oceancloudapi.com/api/v2/ai/text_organization".into(),
            keyhub: "https://keyhub.zijieapi.com/handshake".into(),
            tnc: [
                "tnc3-bjlgy.zijieapi.com",
                "tnc3-alisc1.zijieapi.com",
                "tnc0-aliec2.zijieapi.com",
                "tnc0-bjlgy.zijieapi.com",
            ]
            .iter()
            .map(|h| format!("https://{h}/get_domains/v5/"))
            .collect(),
        }
    }
}

/// 客户端配置。可通过 [`ClientBuilder`](crate::ClientBuilder) 构造，也可从 JSON 反序列化
/// （所有字段可省略，未给出的取默认值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// 服务端地址。
    pub endpoints: Endpoints,
    /// HTTP 请求超时（秒）。
    pub http_timeout_secs: f64,
    /// 固定设备 ID；为 `None` 时每次获取凭据随机生成。
    pub device_id: Option<String>,
    /// 覆盖默认 User-Agent。
    pub user_agent: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            endpoints: Endpoints::default(),
            http_timeout_secs: 15.0,
            device_id: None,
            user_agent: None,
        }
    }
}

impl ClientConfig {
    pub(crate) fn http_timeout(&self) -> Duration {
        Duration::from_secs_f64(self.http_timeout_secs.max(0.1))
    }
}
