# 豆包 WSS ASR 接口技术调研报告

> 调研日期: 2026-03-31
> 调研方法: 通过 Chrome DevTools MCP 对 doubao.com 页面进行实时调试，拦截 WebSocket 连接、分析 JS 源码、抓取网络请求
>
> **2026-09-27 更新**：第 0 节是用 `crates/moli-core/examples/asr_probe.rs` 对真实接口实测得到的结论，与后文冲突时以第 0 节为准。后文中已被推翻的说法就地标注了。

---

## 0. 实测结论（S1，2026-09-27）

测试方法：`scripts/asr-probe.sh`，用 `say -v Tingting` 生成的 16 kHz 语料（`crates/moli-core/fixtures/`），按实时速率发送，裁掉尾部静音，让停止时机正好压在最后一个音节上。

### 0.1 结束方式：发 finish 帧

停止录音时上行发送文本帧 `{"event":"finish"}`，服务端会先推送最后的 `result`，再回 `finish`，随后以 `1000 "1000-"` 关闭。

| 停止方式 | 完整率 | 停止 → 最终结果 |
|---|---|---|
| **发 finish 帧（采用）** | zh_short 19/20，zh_long 5/5，zh_fast 5/5 | p50 ≈ 240 ms，p95 ≈ 590 ms |
| 补 200 ms 静音后等待 | 6/10，且结尾少句号 | — |
| 补静音 + finish 帧 | 5/5 | ≈ 310 ms |
| 直接关闭 WebSocket（旧文档的做法） | 0/5，尾字全丢 | — |
| 什么都不发 | 服务端不会主动 finish，约 10 s 后以 `1000 ""` 关闭 | — |

唯一一次失败：服务端卡住后直接以 `1000 ""` 关闭，没有 `finish`。所以客户端必须设 finish 超时（3 s），超时或异常关闭时退回到最后一次收到的 `result` 文本。

### 0.2 握手

- **`Origin: https://www.doubao.com` 必须带**：不带时每次都是 HTTP 403。
- `User-Agent` 可以不带。
- 查询参数服务端不校验版本：3.38.5 那套和 3.12.3 那套（没有 `doubao_*` 参数）都能用。MoliWhisper 当前用的参数表见 `crates/moli-core/src/asr/params.rs`。
- 一个 Cookie 都不带时，握手返回 **HTTP 200**（不是 101，大概是 WAF 页面），应当按「请求被拦截」处理，不是认证失败。
- 部分边缘节点（遇到过 `71.18.x`）TCP 能连上，TLS 却卡住不动，curl 也一样。客户端需要给每次连接尝试单独设 2–2.5 s 超时，然后重试。同一时段内连接耗时在 0.3–2 s 之间波动。

### 0.3 认证与游客模式

- **没有 `sessionid` 也能识别**：只带非会话 Cookie 时服务端按游客处理，识别结果正常。
- **`sessionid` 无效时握手照样成功**。随后服务端要么先发 `{"code":710022013,"message":"tourist reach limited"}` 再断开，要么直接以 `1000 "2013"` 关闭（reason 是错误码的后 4 位），一条消息都不发。
- 所以登录过期**不能**靠握手状态码判断。判定依据是：错误码 `710022013`，或者关闭原因为 `"2013"`。
- 「一条消息都没收到就被 `1000 "1000-"` 关闭」不代表认证失效，见 0.4。

### 0.4 空闲超时

- 握手完成后大约 **2.5 s 内必须开始发音频**。空闲 2.2 s 后再发音频正常；空闲 2.6 s、3 s、6 s、9 s、12 s、15 s 后再发，服务端都直接以 `1000 "1000-"` 关闭，一条消息都不发。
- 因此**不能预连接**。应该先开麦克风，连接期间的音频先缓存，连上后立即补发。
- 音频停止后，如果不发 finish，服务端约 10 s 后以 `1000 ""` 关闭。

### 0.5 音频与吞吐

- 分片大小不影响结果：1024、4096、16384 字节都正常。
- 一次性快速灌入 12 s 音频，服务端会接收，但处理速度大约是实时，停止到 `finish` 约 6.8 s，3 次里有 1 次没等到 `finish` 就被关闭。补发连接期间缓存的音频（通常不到 2 s）没问题；更长的积压要预期延迟。
- ITN 是开着的：「下午三点」会识别成「下午3点」。

### 0.6 错误码

| code | 含义 |
|---|---|
| `710022013` | tourist reach limited，会话无效或游客额度用完；对应关闭原因 `"2013"` |
| `709599053` / `709599054` | 旧文档记录的 Timeout / Invalid，实测没有遇到过 |

旧文档里的十六进制写法 `0x2A51E74D` / `0x2A51E74E` 换算成十进制是 710010701 / 710010702，和括号里的十进制对不上，以十进制为准。

### 0.7 关闭原因

| code / reason | 含义 |
|---|---|
| `1000 "1000-"` | 正常结束（`finish` 之后），或者握手后空闲过久 |
| `1000 ""` | 服务端放弃了这次会话：音频停止约 10 s，或者服务端卡住 |
| `1000 "2013"` | 会话无效（同 `710022013`） |

---

## 1. 总体架构

豆包 Web 端的语音识别（ASR）采用 **WebSocket 流式传输** 方案：

```
浏览器麦克风 → AudioContext(16kHz) → PCM Int16 → WebSocket binary → 服务端 ASR
                                                              ← JSON text (识别结果)
```

**关键发现**：WSS 连接的认证完全依赖浏览器自动携带的 **httpOnly Cookie**，URL query 参数仅用于客户端标识，不含认证 token。这意味着直接调用 WSS 需要先从 webview 提取 cookie。

---

## 2. WSS 端点

```
wss://ws-samantha.doubao.com/samantha/audio/asr
```

### 2.1 完整 URL 格式

```
wss://ws-samantha.doubao.com/samantha/audio/asr?version_code=20800&language=zh&device_platform=web&aid=497858&real_aid=497858&pkg_type=release_version&device_id={device_id}&pc_version=3.12.3&web_id={web_id}&tea_uuid={tea_uuid}&region=&sys_region=&samantha_web=1&use-olympus-account=1&web_tab_id={web_tab_id}&format=pcm
```

### 2.2 Query 参数说明

| 参数 | 值 | 来源 | 说明 |
|------|-----|------|------|
| `version_code` | `20800` | 固定值 | 客户端版本号 |
| `language` | `zh` | 固定值 | 语言 |
| `device_platform` | `web` | 固定值 | 平台标识 |
| `aid` | `497858` | 固定值 | 应用 ID（豆包 Web） |
| `real_aid` | `497858` | 同 aid | 实际应用 ID |
| `pkg_type` | `release_version` | 固定值 | 包类型 |
| `device_id` | `7623404988207515145` | `localStorage['samantha_web_web_id'].web_id` | 设备标识，首次访问时生成并持久化 |
| `pc_version` | `3.12.3` | 固定值（随版本更新） | PC 客户端版本 |
| `web_id` | `7623404978401396250` | `localStorage['__tea_cache_tokens_497858'].web_id` | Tea SDK 分配的 web_id |
| `tea_uuid` | 同 web_id | 同 web_id | Tea 埋点 UUID |
| `region` | (空) | 固定值 | 区域 |
| `sys_region` | (空) | 固定值 | 系统区域 |
| `samantha_web` | `1` | 固定值 | Samantha Web 标识 |
| `use-olympus-account` | `1` | 固定值 | Olympus 账号体系标识 |
| `web_tab_id` | UUID v4 | 每次页面加载随机生成 | 标签页唯一标识 |
| `format` | `pcm` | 固定值 | 音频格式，始终为 pcm |

**参数来源代码**（`async-infra-input.b55d293d.js`）：
```javascript
// commonParams 来自全局配置，所有 API 请求共享同一套参数
let { getCommonParams } = (0, a.GZ)() ?? {};
let config = {
  commonParams: getCommonParams?.() ?? {},
  // ...
};

// WSS URL 构建：hardcoded endpoint + commonParams + format=pcm
this.socket = new WebSocket(
  buildUrl("wss://ws-samantha.doubao.com/samantha/audio/asr", {
    ...this.config.commonParams,
    format: "pcm"
  })
);
this.socket.binaryType = "arraybuffer";
```

### 2.3 WebSocket 子协议

无。直接 `new WebSocket(url)` 不传 protocols 参数。

---

## 3. 认证机制

### 3.1 Cookie 认证（核心）

WSS 连接的认证通过浏览器自动携带的 `.doubao.com` 域 cookie 完成。关键 cookie 均为 **httpOnly**，JS 无法直接读取。

**关键认证 Cookie**（从 HTTP 请求头中抓取）：

| Cookie | 示例值 | 说明 |
|--------|--------|------|
| `sessionid` | `4ff1753345ceacf7b4f378da7b377373` | **核心会话 ID**，httpOnly |
| `sessionid_ss` | 同 sessionid | 同上（SS 变体） |
| `sid_tt` | 同 sessionid | TT 平台会话 ID |
| `sid_guard` | `{sessionid}\|{timestamp}\|{ttl}\|{expiry}` | 会话守卫，含过期时间 |
| `uid_tt` | `1c70553df6fbd3bd812d9a5316f19bb6` | 用户 ID token |
| `uid_tt_ss` | 同 uid_tt | 同上（SS 变体） |
| `odin_tt` | (128字符hex) | Odin 认证 token |
| `sid_ucp_v1` | base64 编码 | UCP 会话 token |
| `ssid_ucp_v1` | 同 sid_ucp_v1 | 同上（SS 变体） |
| `ttwid` | `1\|{token}\|{timestamp}\|{hash}` | TT Web ID |
| `multi_sids` | `{entity_id}:{sessionid}` | 多会话映射 |
| `session_tlb_tag` | `sttt\|8\|{encoded}` | 会话负载均衡标签 |

**非 httpOnly Cookie**（JS 可读取）：

| Cookie | 说明 |
|--------|------|
| `passport_csrf_token` | CSRF token |
| `s_v_web_id` | Web ID 校验 |
| `flow_cur_user_sec_id` | 用户安全 ID（Base64 编码） |

### 3.2 WKWebView 中如何提取 Cookie

在当前 MoliWhisper 的 WKWebView 架构中，cookie 由 `WKWebsiteDataStore.default()` 管理。要提取 httpOnly cookie 用于直接 WSS 调用：

```swift
// 方法 1: 通过 WKHTTPCookieStore 获取所有 cookie（包括 httpOnly）
let cookieStore = webView.configuration.websiteDataStore.httpCookieStore
cookieStore.getAllCookies { cookies in
    let doubaoCookies = cookies.filter { $0.domain.contains("doubao.com") }
    // doubaoCookies 包含 sessionid、sid_tt 等 httpOnly cookie
}
```

### 3.3 直接调用 WSS 时的 Cookie 设置

WebSocket 标准不支持自定义请求头。在非浏览器环境中建立 WSS 连接时，需要通过以下方式传递 cookie：

- **URLSession (Swift)**: 通过 `HTTPCookieStorage` 或 `URLRequest` 的 `setValue(_:forHTTPHeaderField:)` 设置 Cookie header
- **URLSessionWebSocketTask**: 可以在创建前通过 URLRequest 设置 cookie header

```swift
var request = URLRequest(url: wssURL)
let cookieString = cookies.map { "\($0.name)=\($0.value)" }.joined(separator: "; ")
request.setValue(cookieString, forHTTPHeaderField: "Cookie")
request.setValue("https://www.doubao.com", forHTTPHeaderField: "Origin")
let task = URLSession.shared.webSocketTask(with: request)
```

---

## 4. 音频格式规范

### 4.1 录制参数

| 参数 | 值 |
|------|-----|
| 采样率 | **16000 Hz** (16kHz) |
| 通道数 | **1** (单声道) |
| 位深 | **16 bit** (Int16) |
| 编码 | **PCM** (线性 PCM, Little-Endian) |
| 字节序 | **Little-Endian** |

### 4.2 AudioContext 配置

```javascript
// 创建 AudioContext，固定 16kHz 采样率
let audioContext = new AudioContext({ sampleRate: 16000 });

// 方案 A: ScriptProcessorNode（旧方案，仍在使用）
let processor = audioContext.createScriptProcessor(8192, 1, 1); // bufferSize=8192, 1in, 1out

// 方案 B: AudioWorklet（新方案）
// bufferSize=2048, 每 ~128ms 发送一次
```

### 4.3 Float32 → Int16 PCM 转换

豆包前端使用以下算法将 AudioBuffer 的 Float32 数据转为 Int16 PCM：

```javascript
function floatTo16BitPCM(float32Array, dataView) {
    for (let i = 0; i < float32Array.length; i++) {
        let sample = float32Array[i];
        // 非对称缩放：负值 * 32768，正值 * 32767
        sample = sample < 0 ? sample * 32768 : sample * 32767;
        dataView.setInt16(i * 2, sample, true); // true = little-endian
    }
}
```

### 4.4 发送方式

- 数据类型: `Uint8Array`（从 Int16 PCM 的 ArrayBuffer 创建）
- WebSocket `binaryType`: `"arraybuffer"`
- 每次发送的数据大小:
  - ScriptProcessor 方案: **16384 bytes** (8192 samples × 2 bytes/sample)
  - AudioWorklet 方案: **4096 bytes** (2048 samples × 2 bytes/sample)
- 无任何帧头/封装，直接发送裸 PCM 数据

---

## 5. 消息协议

### 5.1 客户端 → 服务端（上行）

音频是二进制帧；结束时另发一个文本帧 `{"event":"finish"}`。
- 无初始握手/配置消息
- 连接建立后立即开始发送 PCM 音频数据
- ~~无结束标记帧，客户端直接关闭 WebSocket 来结束录音~~ **实测更正**：结束时要发送文本帧 `{"event":"finish"}`，直接关闭会丢掉尾字（见 0.1）

### 5.2 服务端 → 客户端（下行）

JSON 文本消息，有两种事件类型：

#### result 事件（实时识别结果）

```json
{
    "event": "result",
    "result": {
        "Text": "你好，这是识别到的文字"
    },
    "code": 0,
    "message": ""
}
```

- 持续发送，频率与音频数据发送频率大致相同
- `Text` 为空字符串时表示未识别到有效语音
- `Text` 内容会随着说话不断更新（非增量，每次都是完整文本）
- `code: 0` 表示成功

#### finish 事件（识别完成）

```json
{
    "event": "finish",
    "result": null,
    "code": 0,
    "message": ""
}
```

- 服务端在收到客户端的 `{"event":"finish"}` 后发送（实测：不发 finish 时服务端不会主动发送，见 0.1）
- 客户端收到后执行 `stopWsConnection()`

### 5.3 错误码

| code | 名称 | 说明 |
|------|------|------|
| `0` | Success | 成功 |
| `0x2A51E74D` (709599053) | Timeout | 超时（十六进制与十进制对不上，见 0.6） |
| `0x2A51E74E` (709599054) | Invalid | 无效请求 |
| 710022013 | tourist reach limited | 实测：会话无效或游客额度用完 |

### 5.4 WebSocket 关闭

- 正常关闭码: `1000`
- 关闭原因: `"1000-"`（其他原因见 0.7）
- 客户端主动关闭（用户停止录音）或服务端发送 finish 后关闭

---

## 6. 完整调用流程

```
1. 获取麦克风权限
   navigator.mediaDevices.getUserMedia({ audio: true })

2. 构建 WSS URL
   url = "wss://ws-samantha.doubao.com/samantha/audio/asr?" + commonParams + "&format=pcm"

3. 建立 WebSocket 连接
   ws = new WebSocket(url)
   ws.binaryType = "arraybuffer"
   // 浏览器自动携带 .doubao.com 域的 cookie（含 httpOnly session cookie）

4. 创建 AudioContext (sampleRate: 16000)

5. WebSocket open 后，开始录音
   - ScriptProcessor: bufferSize=8192, 每帧 16384 bytes
   - 或 AudioWorklet: bufferSize=2048, 每帧 4096 bytes

6. 持续发送 PCM 数据
   ws.send(uint8Array)  // 裸 Int16 LE PCM

7. 持续接收识别结果
   { "event": "result", "result": { "Text": "..." } }

8. 结束录音
   - 用户主动停止: 客户端 ws.close()
   - 服务端超时: 收到 { "event": "finish" } 后客户端关闭

9. 重连机制
   - 连接失败后 2 秒重试
   - 最多重试 10 次
   - 5 秒连接超时
```

---

## 7. 参数提取方案（从 WebView 到直接调用）

### 7.1 需要从 WebView 提取的数据

| 数据 | 提取方式 | 更新频率 |
|------|----------|----------|
| Session Cookie (sessionid, sid_tt 等) | `WKHTTPCookieStore.getAllCookies()` | 登录时获取，30天有效 (sid_guard) |
| device_id | `webView.evaluateJavaScript("localStorage.getItem('samantha_web_web_id')")` | 首次生成后不变 |
| web_id / tea_uuid | `webView.evaluateJavaScript("localStorage.getItem('__tea_cache_tokens_497858')")` | 首次生成后不变 |
| web_tab_id | 随机生成 UUID v4 | 每次连接可重新生成 |

### 7.2 固定参数（无需提取）

```swift
let fixedParams: [String: String] = [
    "version_code": "20800",
    "language": "zh",
    "device_platform": "web",
    "aid": "497858",
    "real_aid": "497858",
    "pkg_type": "release_version",
    "pc_version": "3.12.3",
    "region": "",
    "sys_region": "",
    "samantha_web": "1",
    "use-olympus-account": "1",
    "format": "pcm"
]
```

> 注意: `version_code` 和 `pc_version` 可能随豆包 Web 版本更新而变化

### 7.3 直接调用 WSS 的 Swift 伪代码

```swift
// 1. 从 WKWebView 提取 cookie
let cookies = await webView.configuration.websiteDataStore.httpCookieStore.allCookies()
let doubaoCookies = cookies.filter { $0.domain.contains("doubao.com") }

// 2. 从 localStorage 提取 device_id, web_id
let deviceIdJSON = await webView.evaluateJavaScript(
    "localStorage.getItem('samantha_web_web_id')"
) as? String
let deviceId = parseJSON(deviceIdJSON)?["web_id"]

let teaCacheJSON = await webView.evaluateJavaScript(
    "localStorage.getItem('__tea_cache_tokens_497858')"
) as? String
let webId = parseJSON(teaCacheJSON)?["web_id"]

// 3. 构建 WSS URL
var components = URLComponents(string: "wss://ws-samantha.doubao.com/samantha/audio/asr")!
components.queryItems = [
    URLQueryItem(name: "version_code", value: "20800"),
    URLQueryItem(name: "language", value: "zh"),
    URLQueryItem(name: "device_platform", value: "web"),
    URLQueryItem(name: "aid", value: "497858"),
    URLQueryItem(name: "real_aid", value: "497858"),
    URLQueryItem(name: "pkg_type", value: "release_version"),
    URLQueryItem(name: "device_id", value: deviceId),
    URLQueryItem(name: "pc_version", value: "3.12.3"),
    URLQueryItem(name: "web_id", value: webId),
    URLQueryItem(name: "tea_uuid", value: webId),
    URLQueryItem(name: "region", value: ""),
    URLQueryItem(name: "sys_region", value: ""),
    URLQueryItem(name: "samantha_web", value: "1"),
    URLQueryItem(name: "use-olympus-account", value: "1"),
    URLQueryItem(name: "web_tab_id", value: UUID().uuidString),
    URLQueryItem(name: "format", value: "pcm"),
]

// 4. 创建 WebSocket 请求（附带 cookie）
var request = URLRequest(url: components.url!)
let cookieHeader = doubaoCookies.map { "\($0.name)=\($0.value)" }.joined(separator: "; ")
request.setValue(cookieHeader, forHTTPHeaderField: "Cookie")
request.setValue("https://www.doubao.com", forHTTPHeaderField: "Origin")

// 5. 建立 WebSocket
let wsTask = URLSession.shared.webSocketTask(with: request)
wsTask.resume()

// 6. 使用 AVAudioEngine 录制 16kHz 单声道 PCM 并发送
// ... (参见音频格式规范)
```

---

## 8. 注意事项

1. **Cookie 有效期**: `sid_guard` 显示 session 有效期为 30 天 (2592000 秒)，过期后需要重新登录
2. **Cookie 刷新**: 豆包页面通过 `/passport/token/beat/web/` 接口维持 session 活跃，直接调用需考虑 session 保活
3. **无 msToken/a_bogus**: WSS 连接不需要 `msToken` 和 `a_bogus` 防爬参数（这两个只在 HTTP API 请求中需要）
4. **无初始握手**: 连接建立后直接发送 PCM 二进制数据，无需先发送任何配置/初始化文本消息
5. **Origin 头**: WebSocket 握手必须包含 `Origin: https://www.doubao.com`（实测：不带时 HTTP 403）
6. **API 版本变化**: `version_code` 和 `pc_version` 会随着豆包更新而变化。实测服务端不校验这两个值，MoliWhisper 用常量表加配置覆盖（`asr.param_overrides`）
7. **AudioASR HTTP API**: 源码中还存在一个 `AudioASR` HTTP GET API（同路径 `/samantha/audio/asr`），支持额外参数 `codec, rate, bits, channel, language, scene`，但当前 Web 端未使用该 HTTP 路径，仅使用 WSS

---

## 9. 用户身份信息

通过 `/alice/profile/self` API 可获取当前登录用户信息：

```
POST https://www.doubao.com/alice/profile/self?{commonParams}
Body: {"visit_id":"29064417512292","avatar_format":"png"}
```

响应：
```json
{
    "code": 0,
    "msg": "",
    "data": {
        "profile_brief": {
            "id": "857628872613378",
            "entity_id": "29064417512292",
            "nickname": "用户qYwiND",
            "user_name": "817544585"
        }
    }
}
```

其中 `entity_id` 即 `visit_id`，也是 `multi_sids` cookie 和 `flow_tea_user_id` localStorage 中的用户标识。

---

## 10. localStorage 关键数据索引

| Key | 内容 | 用途 |
|-----|------|------|
| `samantha_web_web_id` | `{"web_id":"...", "tt_wid":"..."}` | device_id 来源 |
| `__tea_cache_tokens_497858` | `{"web_id":"...", "user_unique_id":"..."}` | web_id / tea_uuid 来源 |
| `flow_tea_user_id` | `"29064417512292"` | 用户 entity_id |
| `flow_web_has_login` | `"true"` | 登录状态 |
| `SLARDARflow_web` | Base64 编码 JSON | 含 userId, deviceId, expires |
