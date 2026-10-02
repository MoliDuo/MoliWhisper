"""``moli-asr-server``: serves Qwen3-ASR (or ``--fake``) over WebSocket."""

from __future__ import annotations

import argparse
import logging
import os

import uvicorn

from .app import create_app
from .engine import Engine, FakeEngine


def main() -> None:
    p = argparse.ArgumentParser(prog="moli-asr-server")
    p.add_argument("--host", default="127.0.0.1", help="listen address (0.0.0.0 for all interfaces)")
    p.add_argument("--port", type=int, default=int(os.environ.get("MOLI_ASR_PORT", "8765")))
    p.add_argument("--model", default=os.environ.get("MOLI_ASR_MODEL", "Qwen/Qwen3-ASR-1.7B"))
    p.add_argument("--fake", action="store_true", help="no model: answer with the length of the audio (for development)")
    p.add_argument("--gpu-memory-utilization", type=float, default=float(os.environ.get("MOLI_GPU_MEMORY_UTILIZATION", "0.8")))
    p.add_argument("--chunk-size-sec", type=float, default=2.0, help="audio is decoded in chunks of this length")
    p.add_argument("--unfixed-chunk-num", type=int, default=2)
    p.add_argument("--unfixed-token-num", type=int, default=5)
    p.add_argument("--language", default=os.environ.get("MOLI_ASR_LANGUAGE") or None, help="force a language, e.g. Chinese (default: detect)")
    p.add_argument("--context", default=os.environ.get("MOLI_ASR_CONTEXT", ""), help="context text given to the model, e.g. names and terms")
    args = p.parse_args()

    logging.basicConfig(level=logging.INFO)
    token = os.environ.get("MOLI_ASR_TOKEN", "")
    engine: Engine
    if args.fake:
        engine = FakeEngine()
    else:
        from .engine import QwenEngine

        engine = QwenEngine(
            args.model,
            gpu_memory_utilization=args.gpu_memory_utilization,
            chunk_size_sec=args.chunk_size_sec,
            unfixed_chunk_num=args.unfixed_chunk_num,
            unfixed_token_num=args.unfixed_token_num,
            language=args.language,
            context=args.context,
        )
    if not token:
        logging.warning("MOLI_ASR_TOKEN is not set: anyone who can reach the port can use the server")
    uvicorn.run(create_app(engine, token), host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
