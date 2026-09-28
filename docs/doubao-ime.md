# 豆包输入法 ASR 协议笔记

> 整理日期：2026-09-28
> 来源：对豆包输入法 Mac 版（1.0.1，version_code 1000103）的观测，以及 `crates/moli-core/examples/ime_probe.rs` 对线上接口的实测。
> 实现：`crates/moli-core/src/doubao/ime/`。接口是私有的，随时可能变；与代码冲突时以代码为准。

免登录。客户端自报一个随机设备 id，凭据在运行时获取、只放内存，过期或被拒就重新拿。

## 1. 接口地址

| 用途 | 方法 | 地址 |
|---|---|---|
| 远程配置（含 app_key） | GET | `https://is.snssdk.com/service/settings/v3/` |
| app_key 换 token | POST | `https://ime.oceancloudapi.com/api/v1/user/get_config` |
| 识别 WebSocket | WSS | `wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws` |
| 文字整理 | POST | `https://ime.oceancloudapi.com/api/v2/ai/text_organization` |
| 版本列表 | GET | `https://ime.doubao.com/api/v1/version/list` |

所有 HTTP 请求都带 `User-Agent: DoubaoIme/1.0.1`。`aid` 固定为 `685343`。测试时可以用 `ImeClient::with_endpoints` 把这些地址换成 mock。

## 2. 凭据

1. **app_key**：`GET settings?aid=685343&device_id=<id>&os=mac&channel=release`，取 `data.settings.asr_config.app_key`。
2. **sami_token**：`POST get_config`，body 为 `{"sami_app_key": <app_key>, "device_id": <id>, "aid": 685343}`，取 `Data.sami_token`（有时拼成小写的 `data`）。
3. token 是 JWT。按 `exp − iat` 算有效期，提前 10 分钟续；解析不了时按 30 分钟续。

**设备 id**：16 位十进制数字，本地随机生成，存在应用自己的配置里。不要借用官方输入法的设备 id 或 token。

## 3. 识别 WebSocket

### 3.1 握手

查询参数：`app_key`、`aid=685343`、`device_id`、`device_platform=mac`。

请求头：

| 头 | 值 | 说明 |
|---|---|---|
| `Proto-Version` | `v2` | |
| `User-Agent` | `DoubaoIme/1.0.1` | |
| `X-Api-Resource-Id` | `original.sami.ASR` | |
| `x-custom-keepalive` | `true` | 输入法自己就这么要：每 20 s ping 一次，连接保留 2 h |
| `x-keepalive-interval` | `20` | |
| `x-keepalive-timeout` | `7200` | |

### 3.2 帧格式

全部是二进制帧，内容为 protobuf（`mammon_internal.WebSocketRequest` / `WebSocketResponse`）。schema 写在 `wire.rs` 的模块文档里。实现用 `prost` derive，不需要 `protoc`。单元测试会拿官方 protobuf 运行时产出的字节做逐字节比对。

请求里每帧都带 `appkey`。`StartSession` 和 `FinishSession` 额外带 `token`。

### 3.3 事件时序

```text
连接 → StartTask → TaskStarted
      ┌─ StartSession(token, namespace="ASR", payload) → SessionStarted
      │  TaskRequest(audio_data, payload="{}") × N      ← 结果帧（event 为空）
      │  FinishSession(token)                          → 剩余结果 … SessionFinished
      └─ 同一个 task 上可以接着开下一个 session
```

- `StartSession` 的 payload 按输入法原样发送：`{"extra":{"enable_punctuation":true}}`。静音超时、句长等参数都交给服务端决定。
- 音频为 16 kHz 单声道 s16le PCM，每帧 200 ms（6400 字节）。
- 建连加 `StartTask` 要几秒，在已开的 task 上开 session 只要几百 ms。所以两次听写之间会保持一条连接（见 `pool.rs`）：
  - 每 20 s ping 一次；
  - 30 s 没收到任何东西就判定连接已死；
  - 110 分钟后，或 token 该续期时，主动换新连接。

### 3.4 状态码

| `status_code` | 含义 |
|---|---|
| `20000000` | OK |
| `40000000` | 会话失败 |
| `40000012` | 数据非法 |
| `40100003` | 需要先 `StartTask` |
| `50700000` | 服务发现失败：这个设备 id 路由不到（见 3.6） |

出错时 `event` 为 `TaskFailed` 或 `SessionFailed`。

### 3.5 结果与分段

结果帧的 `event` 为空，payload 为：

```json
{"results":[{"text":"…","index":0,"start_time":0.0,"end_time":1.4,"confidence":0.99,"alternatives":[…]}]}
```

- `text` 是**当前这一段**到目前为止的全文，不是整场的累积全文，也不是增量。
- 服务端会把语音切成若干段。整场文本等于各段按顺序拼接。
- 停顿之后，新的一段用下一个 `index`。
- 连续说话约 24 s 不停顿时，服务端会开一个 **`index` 相同**的新段，这时只能靠 `start_time` 区分。同一段的 `start_time` 在识别过程中会漂移最多约 0.5 s，所以实现里用 1.5 s 的容差判断两条结果是否属于同一段（见 `transcript.rs`）。

### 3.6 路由不到的设备 id

有些设备 id 服务端永远路由不到。表现是 `StartSession` 能成功，但一发音频立刻 `SessionFailed`，状态码 `50700000`。应对方式：

- 预热连接时先发 100 ms 静音做一次探测，失败就换一个新设备 id，最多换 3 次。
- 正在进行的会话中途失败（断网、会话失败，或者就是这种情况）时，开一个新会话，从最后一段（可能没说完）的起点前 0.3 s 开始，把已录的音频重新发一遍。每次录音最多重放 3 次。如果失败原因是路由不到，重放前先换一个新设备 id。
- 换了新 id 会回调通知应用，由应用保存下来。

## 4. 文字整理

`POST text_organization`，body 为 `{"scene": 6, "query": <口语文本>}`（scene 6 是输入法语音输入用的场景）。

返回：

```json
{"code": 0, "msg": "…", "data": {"content": "…", "no_rewrite": false}}
```

- `code` 非 0 表示失败。
- `no_rewrite` 为 true 表示服务端认为原文不需要改动。
- 不需要凭据。

## 5. 版本检查

`GET version/list?aid=685343&platform=macos&channel=release&version_code=1000103`，返回 `{"code":0,"data":{"list":[{"version_name","version_code"}…]}}`。

应用启动时查一次。发现有比自报版本更新的输入法版本就打一条警告日志，因为服务端将来可能拒绝旧版本。
