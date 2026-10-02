import json

import pytest
from fastapi.testclient import TestClient
from starlette.testclient import WebSocketDenialResponse

from moli_asr_server.app import create_app
from moli_asr_server.engine import FakeEngine

SECOND = b"\x00\x00" * 16_000  # one second of silence


def client(token: str = "") -> TestClient:
    return TestClient(create_app(FakeEngine(), token))


def test_partial_then_final_then_close():
    with client().websocket_connect("/v1/stream") as ws:
        ws.send_bytes(SECOND)
        assert json.loads(ws.receive_text()) == {"type": "partial", "text": "已收到 1.0 秒音频"}
        ws.send_bytes(SECOND)
        assert json.loads(ws.receive_text())["text"] == "已收到 2.0 秒音频"
        ws.send_text('{"type":"finish"}')
        assert json.loads(ws.receive_text()) == {"type": "final", "text": "已收到 2.0 秒音频。"}


def test_unchanged_text_is_not_repeated():
    with client().websocket_connect("/v1/stream") as ws:
        ws.send_bytes(b"")  # nothing new: the fake still says 0.0 once
        assert json.loads(ws.receive_text())["text"] == "已收到 0.0 秒音频"
        ws.send_bytes(b"")
        ws.send_text('{"type":"finish"}')
        assert json.loads(ws.receive_text())["type"] == "final"


def test_odd_byte_is_ignored():
    with client().websocket_connect("/v1/stream") as ws:
        ws.send_bytes(SECOND + b"\x01")
        assert json.loads(ws.receive_text())["text"] == "已收到 1.0 秒音频"


def test_token_is_required():
    c = client("secret")
    with pytest.raises(WebSocketDenialResponse) as e:
        with c.websocket_connect("/v1/stream"):
            pass
    assert e.value.status_code == 401
    with pytest.raises(WebSocketDenialResponse):
        with c.websocket_connect("/v1/stream", headers={"Authorization": "Bearer wrong"}):
            pass
    with c.websocket_connect("/v1/stream", headers={"Authorization": "Bearer secret"}) as ws:
        ws.send_text('{"type":"finish"}')
        assert json.loads(ws.receive_text())["type"] == "final"


def test_healthz():
    assert client().get("/healthz").json() == {"ok": True, "model": "fake"}
    c = client("secret")
    assert c.get("/healthz").status_code == 401
    assert c.get("/healthz", headers={"Authorization": "Bearer secret"}).status_code == 200
