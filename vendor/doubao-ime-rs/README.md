# doubao-ime (Rust)

豆包输入法 **语音识别**（WAV → 文本）与 **文字整理**（口语 → 书面语）的异步 Rust 客户端。

> ⚠️ **非官方实现。** 基于对豆包输入法（Mac 版）私有接口的逆向分析，并非字节跳动官方 SDK。
> 接口、鉴权与版本参数随时可能变化而失效；`aid`、User-Agent 等取自被观测的客户端。
> 请自行评估是否符合相关服务条款。需要稳定、正式授权的能力，请使用火山引擎官方豆包语音 API。

所有参数在运行时获取或随机生成：**无内置密钥、不读写本地状态**。

## 工作区结构

| Crate | 说明 |
|------|------|
| `doubao-ime` | 核心库：异步 API + 可选同步封装（`blocking` 特性，默认开启） |
| `doubao-ime-cli` | 命令行工具，二进制名 `doubao-ime` |
| `doubao-ime-ffi` | C ABI（`cdylib` + `staticlib`），供 C/C++/Python/Go/… 调用 |

## 能力一览

- **文字整理** `organize` / `organize_stream`（SSE）—— 无需鉴权。
- **语音识别** `recognize_file` / `recognize_pcm`，或用 `asr_session` 做流式识别并实时接收增量结果。
  - 自动完成 `app_key → sami_token` 凭据获取；`compat` 模式额外走 keyhub 握手取票据。
- **底层原语**：`keyhub_handshake`、`fetch_tnc_config`、信封加密 `envelope_organize` / `decrypt_envelope`。
- 每个接口地址都可通过 `Endpoints` 覆盖（便于对接测试服务器或代理）。

## 库用法

```rust
use doubao_ime::{Client, AsrOptions};

#[tokio::main]
async fn main() -> doubao_ime::Result<()> {
    let client = Client::new()?;

    // 文字整理
    let out = client.organize("嗯那个我想问一下明天几点开会").await?;
    println!("{}", out.content);

    // 语音识别（16kHz 单声道 16bit WAV）
    let text = client.recognize_file("audio.wav", AsrOptions::default()).await?;
    println!("{text}");
    Ok(())
}
```

流式识别 + 实时结果、同步封装等见 `doubao-ime/examples/` 与 crate 文档（`cargo doc -p doubao-ime --open`）。

### 同步（阻塞）API

默认启用的 `blocking` 特性提供 `doubao_ime::blocking::Client`，内部持有 tokio 运行时：

```rust
let client = doubao_ime::blocking::Client::new()?;
println!("{}", client.organize("嗯那个明天开会")?.content);
```

只用异步、想去掉运行时依赖：`doubao-ime = { version = "0.1", default-features = false }`。

## 命令行

```console
$ doubao-ime organize "嗯那个我想问一下明天几点开会"
$ doubao-ime organize --stream "口语文本"
$ doubao-ime asr recording.wav            # 16kHz/单声道/16bit
$ doubao-ime asr --status                 # 只获取并展示本次凭据
$ doubao-ime keyhub
$ doubao-ime tnc -o tnc.json
$ doubao-ime envelope --config tnc.json --organize "文本"
$ doubao-ime -v ...                       # 调试日志
```

## 从其他语言调用（C ABI）

```console
$ cargo build -p doubao-ime-ffi --release
# 产物：target/release/libdoubao_ime.{so,dylib,a}
```

```c
#include "doubao_ime.h"          // doubao-ime-ffi/include/
#include <stdio.h>

int main(void) {
    DbaoClient *c = dbao_client_new();
    char *out = NULL;
    if (dbao_organize(c, "嗯那个明天开会", &out) == 0) {
        printf("%s\n", out);
        dbao_string_free(out);
    } else {
        fprintf(stderr, "%s\n", dbao_last_error());
    }
    dbao_client_free(c);
}
```

编译链接完整示例见 `doubao-ime-ffi/examples/organize.c`。约定：返回 `0` 成功、负值为错误类别（见头文件 `DbaoErrorKind`）；返回的字符串用 `dbao_string_free` 释放；`dbao_last_error()` 取当前线程最近错误。

## 构建与测试

```console
$ cargo build --workspace
$ cargo test -p doubao-ime      # 含离线假 WebSocket 服务器的端到端测试，不触网
$ cargo doc -p doubao-ime --open
```

MSRV：Rust 1.80。

## 与协议相关的实现要点

- ASR 走 WebSocket + protobuf（`mammon_internal.WebSocketRequest/Response`），用 `prost` 派生，无需 `protoc`；单元测试与官方 protobuf 运行时产出的字节逐字节比对。
- 识别结果帧的 `event` 为空字符串，`text` 字段为**累积全文**而非增量。
- 信封解密：`ChaCha20`（IETF，12 字节 nonce，计数器从 0 开始）——等价于原实现的 `00000000 || nonce` 16 字节构造。

## 免责声明

仅供研究与互操作性学习。风险自负。
