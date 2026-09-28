//! 异步客户端入口。

use std::sync::Arc;
use std::time::Duration;

use crate::config::{ClientConfig, Endpoints, UA};
use crate::error::Result;

/// 异步客户端。内部持有连接池，`Clone` 开销很小，可在任务间共享。
///
/// ```no_run
/// # async fn demo() -> doubao_ime::Result<()> {
/// let client = doubao_ime::Client::new()?;
/// println!("{}", client.organize("嗯那个我想问一下明天几点开会").await?.content);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) config: Arc<ClientConfig>,
}

impl Client {
    /// 使用默认配置创建客户端。
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// 返回构造器。
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// 从完整配置创建客户端。
    pub fn from_config(config: ClientConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.http_timeout())
            .user_agent(config.user_agent.clone().unwrap_or_else(|| UA.to_owned()))
            .build()?;
        Ok(Self {
            http,
            config: Arc::new(config),
        })
    }

    /// 当前配置。
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub(crate) fn endpoints(&self) -> &Endpoints {
        &self.config.endpoints
    }

    pub(crate) fn device_id(&self) -> String {
        self.config
            .device_id
            .clone()
            .unwrap_or_else(crate::util::gen_device_id)
    }
}

/// [`Client`] 构造器。
#[derive(Debug, Default, Clone)]
pub struct ClientBuilder {
    config: ClientConfig,
}

impl ClientBuilder {
    /// 覆盖全部服务端地址。
    pub fn endpoints(mut self, endpoints: Endpoints) -> Self {
        self.config.endpoints = endpoints;
        self
    }

    /// HTTP 请求超时。
    pub fn http_timeout(mut self, timeout: Duration) -> Self {
        self.config.http_timeout_secs = timeout.as_secs_f64();
        self
    }

    /// 使用固定设备 ID（默认每次获取凭据时随机生成）。
    pub fn device_id(mut self, id: impl Into<String>) -> Self {
        self.config.device_id = Some(id.into());
        self
    }

    /// 覆盖默认 User-Agent。
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.config.user_agent = Some(ua.into());
        self
    }

    /// 构建客户端。
    pub fn build(self) -> Result<Client> {
        Client::from_config(self.config)
    }
}
