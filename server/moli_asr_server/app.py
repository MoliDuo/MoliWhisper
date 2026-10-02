"""The WebSocket protocol (v1) on top of an engine.

Client to server: binary frames of 16 kHz mono s16le PCM, then the text frame
``{"type":"finish"}``. Server to client: ``{"type":"partial","text":…}`` with
the full transcript so far whenever it changes, then ``{"type":"final",…}``
and a normal close; ``{"type":"error","message":…}`` on failure.
"""

from __future__ import annotations

import asyncio
import hmac
import json
import logging
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from fastapi import FastAPI, Request, Response, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse

from .engine import Engine

log = logging.getLogger("moli_asr_server")


def _authorized(header: str | None, token: str) -> bool:
    if not token:
        return True
    return header is not None and hmac.compare_digest(header, f"Bearer {token}")


def create_app(engine: Engine, token: str = "") -> FastAPI:
    app = FastAPI(title="moli-asr-server")
    # One GPU: every call into the model goes through one thread, one at a time.
    executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="asr")
    gpu = asyncio.Lock()

    async def run(fn, *args):
        async with gpu:
            return await asyncio.get_running_loop().run_in_executor(executor, fn, *args)

    @app.get("/healthz")
    async def healthz(request: Request):
        if not _authorized(request.headers.get("authorization"), token):
            return JSONResponse({"ok": False, "error": "unauthorized"}, status_code=401)
        return {"ok": True, "model": engine.name}

    @app.websocket("/v1/stream")
    async def stream(ws: WebSocket):
        if not _authorized(ws.headers.get("authorization"), token):
            # Refuse the upgrade with a plain 401 so the client can tell.
            await ws.send_denial_response(Response("unauthorized", status_code=401))
            return
        await ws.accept()
        session = await run(engine.new_stream)
        last = ""
        try:
            while True:
                msg = await ws.receive()
                if msg["type"] == "websocket.disconnect":
                    return
                if (data := msg.get("bytes")) is not None:
                    pcm = np.frombuffer(data[: len(data) // 2 * 2], dtype="<i2")
                    text = await run(session.feed, pcm)
                    if text != last:
                        last = text
                        await ws.send_text(json.dumps({"type": "partial", "text": text}, ensure_ascii=False))
                elif (text_frame := msg.get("text")) is not None:
                    if _is_finish(text_frame):
                        final = await run(session.finish)
                        await ws.send_text(json.dumps({"type": "final", "text": final}, ensure_ascii=False))
                        await ws.close(code=1000)
                        return
        except WebSocketDisconnect:
            return
        except Exception as e:  # report it to the client, then drop the session
            log.exception("session failed")
            try:
                await ws.send_text(json.dumps({"type": "error", "message": str(e)}, ensure_ascii=False))
                await ws.close(code=1011)
            except Exception:
                pass

    return app


def _is_finish(frame: str) -> bool:
    try:
        return json.loads(frame).get("type") == "finish"
    except (ValueError, AttributeError):
        return False
