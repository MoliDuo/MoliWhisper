# moli-asr-server

给 MoliWhisper 用的自建语音识别服务：在一台 NVIDIA 显卡的机器上跑 [Qwen3-ASR-1.7B](https://github.com/QwenLM/Qwen3-ASR)，
Mac 上的 MoliWhisper 通过 WebSocket 把音频流过来，边说边出字。

官方 `vllm serve` 只有整段识别的接口，没有流式；这里用 `qwen-asr` 自带的流式 API
（`init_streaming_state` / `streaming_transcribe` / `finish_streaming_transcribe`）包了一层很薄的 WebSocket 服务。

## 用 Docker 运行（推荐）

服务器上需要：NVIDIA 驱动、Docker、[NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)（让容器能用显卡）。在仓库根目录：

```bash
cp .env.example .env          # 把 MOLI_ASR_TOKEN 改成一串随机字符：openssl rand -hex 24
docker login ghcr.io -u <GitHub 用户名>   # 只需一次，见下面的说明
docker compose up -d          # 从 GHCR 拉预构建的镜像
docker compose logs -f asr    # 首次要下载约 4 GB 模型并加载进显存，看到 Uvicorn running 就好了
curl -H "Authorization: Bearer 你的token" http://localhost:8765/healthz
```

- 镜像是私有的，所以第一次要登录 GHCR：在 GitHub → Settings → Developer settings → Personal access tokens (classic) 新建一个只勾 `read:packages` 的令牌，当作 `docker login` 的密码。令牌别写进仓库。想免登录，可以在包的设置里把可见性改成 Public。
- 模型存在名为 `models` 的 Docker 卷里，重建容器不会重新下载。服务器访问不了 Hugging Face 时，在 `.env` 里设 `HF_ENDPOINT=https://hf-mirror.com`。
- `restart: unless-stopped`，机器重启后会自动起来。
- 所有可调项都在 [`.env.example`](../.env.example) 里，有注释。
- 想只让 Tailscale 访问：`.env` 里设 `MOLI_BIND=本机的 100.x.y.z`。
- 没有显卡、只想调试客户端：`docker compose --profile fake up -d --build asr-fake`（不下载模型，几十 MB）。
- 更新：`git pull && docker compose pull && docker compose up -d`。镜像由 CI 在 `server/` 有改动时构建发布到 `ghcr.io/moliduo/moliwhisper-asr`（`latest` 和提交 SHA 两个标签）；想自己构建就用 `docker compose up -d --build`。

镜像基于 `nvidia/cuda:12.8` 的 devel 版，里面是 Python 3.12 + vLLM + `qwen-asr`；宿主机驱动要支持 CUDA 12.8（驱动 ≥ 570）。

## 不用 Docker 运行

需要 Linux（或 WSL）、NVIDIA 显卡和 [uv](https://docs.astral.sh/uv/)。

```bash
cd server
uv sync --extra qwen          # 装 qwen-asr[vllm]，体积较大
export MOLI_ASR_TOKEN=换成一串随机字符   # 建议设置；不设则任何能连上端口的人都能用
uv run moli-asr-server --host 0.0.0.0 --port 8765
```

首次启动会从 Hugging Face 下载模型。命令行参数（括号里是 Docker 用的环境变量）：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--model`（`MOLI_ASR_MODEL`） | `Qwen/Qwen3-ASR-1.7B` | 也可以用本地路径或 0.6B |
| `--gpu-memory-utilization`（`MOLI_GPU_MEMORY_UTILIZATION`） | 0.8 | vLLM 占用的显存比例 |
| `--chunk-size-sec` | 2.0 | 每多少秒音频解码一次；越小出字越快，GPU 负担越大 |
| `--unfixed-chunk-num` / `--unfixed-token-num` | 2 / 5 | 流式解码的回退策略，见官方文档 |
| `--language`（`MOLI_ASR_LANGUAGE`） | 自动检测 | 例如 `Chinese` |
| `--context`（`MOLI_ASR_CONTEXT`） | 空 | 给模型的上下文，如人名、术语 |

没有 GPU 时可以用 `--fake` 调试客户端（不加载模型，只回报收到了多长的音频，不需要装 `qwen` 这组依赖）：

```bash
uv sync
uv run moli-asr-server --fake
uv run pytest
```

## 连接

MoliWhisper 设置里选「自建服务器」，地址填 `ws://<主机名>:8765/v1/stream`，Token 填上面的值，点「保存并测试」。
两台机器不在同一个网络时，用 [Tailscale](https://tailscale.com/) 组网，主机名直接写 MagicDNS 名字或 `100.x.y.z`。
不要把端口直接暴露到公网；真要暴露，前面加一层 HTTPS 反向代理，地址写成 `wss://…`。

不开 App 也能手动测：

```bash
cargo run -p moli-core --example selfhost_probe -- ws://主机名:8765/v1/stream 你的token
```

## 协议 v1

`GET /healthz`（带 Token 时需要 `Authorization: Bearer …`）返回 `{"ok":true,"model":"…"}`。

`WS /v1/stream`，每个连接是一次听写。Token 不对时握手返回 HTTP 401。

| 方向 | 帧 | 含义 |
|---|---|---|
| 客户端 → 服务端 | 二进制 | 16 kHz 单声道 s16le PCM，大小不限 |
| 客户端 → 服务端 | 文本 `{"type":"finish"}` | 音频结束 |
| 服务端 → 客户端 | 文本 `{"type":"partial","text":"…"}` | 到目前为止的**完整**文本（不是增量），有变化时才发 |
| 服务端 → 客户端 | 文本 `{"type":"final","text":"…"}` | 最终文本，随后以 1000 关闭 |
| 服务端 → 客户端 | 文本 `{"type":"error","message":"…"}` | 出错，随后关闭 |

## 已知限制

- `qwen-asr` 的流式解码每个 chunk 都会把已收到的全部音频重新送进模型，所以一段话越长，每次解码越慢（近似平方增长）。
  日常的几十秒到一两分钟没有问题；超长录音（客户端上限 5 分钟）请实测。
- GPU 调用是串行的，同时只有一个会话在解码，其余的排队。自己一个人用足够。
