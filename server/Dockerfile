# syntax=docker/dockerfile:1
#
# 两个目标：
#   gpu  （默认）Qwen3-ASR + vLLM，要 NVIDIA 显卡
#   fake 不带模型，只回报音频长度；没有显卡时用来调试客户端
#
#   docker build -t moli-asr-server server            # gpu
#   docker build -t moli-asr-server:fake --target fake server

FROM ghcr.io/astral-sh/uv:latest AS uv

# ---- fake：纯 CPU，几十 MB ----
FROM python:3.12-slim AS fake
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_PROJECT_ENVIRONMENT=/opt/venv \
    UV_LINK_MODE=copy \
    UV_COMPILE_BYTECODE=1 \
    PATH=/opt/venv/bin:$PATH \
    PYTHONUNBUFFERED=1
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project
COPY moli_asr_server ./moli_asr_server
RUN uv sync --frozen --no-dev
RUN useradd --create-home --uid 10001 app
USER app
EXPOSE 8765
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["python", "-m", "moli_asr_server.healthcheck"]
ENTRYPOINT ["moli-asr-server", "--host", "0.0.0.0"]
CMD ["--fake"]

# ---- gpu：带 CUDA 工具链。用 devel 镜像是因为 vLLM 的 triton 等组件运行时要编译小段 C 代码 ----
FROM nvidia/cuda:12.8.1-devel-ubuntu24.04 AS gpu
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=uv /uv /usr/local/bin/uv
# Python 由 uv 装在 /opt/python，所有用户都能读；不用系统自带的 Python
ENV UV_PYTHON_INSTALL_DIR=/opt/python \
    UV_PYTHON_PREFERENCE=only-managed \
    UV_PROJECT_ENVIRONMENT=/opt/venv \
    UV_LINK_MODE=copy \
    UV_COMPILE_BYTECODE=1 \
    PATH=/opt/venv/bin:$PATH \
    PYTHONUNBUFFERED=1 \
    HF_HOME=/models
WORKDIR /app
COPY pyproject.toml uv.lock ./
# 依赖这一层最大（vLLM、PyTorch），单独放前面，改代码时不用重装
RUN uv sync --frozen --no-dev --extra qwen --python 3.12 --no-install-project
COPY moli_asr_server ./moli_asr_server
RUN uv sync --frozen --no-dev --extra qwen --python 3.12
RUN useradd --create-home --uid 10001 app \
    && mkdir /models && chown app /models
USER app
VOLUME /models
EXPOSE 8765
# 首次启动要下载模型并加载进显存，给足时间
HEALTHCHECK --interval=30s --timeout=5s --start-period=10m --retries=3 \
    CMD ["python", "-m", "moli_asr_server.healthcheck"]
ENTRYPOINT ["moli-asr-server", "--host", "0.0.0.0"]
