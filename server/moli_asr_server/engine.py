"""Speech recognition engines: the real model and a fake for development."""

from __future__ import annotations

from typing import Protocol

import numpy as np

SAMPLE_RATE = 16_000


class Stream(Protocol):
    """One dictation. Not thread-safe; calls come one at a time."""

    def feed(self, pcm: np.ndarray) -> str:
        """Takes 16 kHz mono int16 samples; returns the transcript so far."""

    def finish(self) -> str:
        """Flushes the audio still buffered; returns the final transcript."""


class Engine(Protocol):
    name: str

    def new_stream(self) -> Stream: ...


class FakeEngine:
    """Answers with how much audio it has heard. For testing the client."""

    name = "fake"

    def new_stream(self) -> Stream:
        return _FakeStream()


class _FakeStream:
    def __init__(self) -> None:
        self.samples = 0

    def feed(self, pcm: np.ndarray) -> str:
        self.samples += len(pcm)
        return f"已收到 {self.samples / SAMPLE_RATE:.1f} 秒音频"

    def finish(self) -> str:
        return f"已收到 {self.samples / SAMPLE_RATE:.1f} 秒音频。"


class QwenEngine:
    """Qwen3-ASR on vLLM, through the model's own streaming API."""

    def __init__(
        self,
        model: str,
        *,
        gpu_memory_utilization: float,
        chunk_size_sec: float,
        unfixed_chunk_num: int,
        unfixed_token_num: int,
        language: str | None,
        context: str,
    ) -> None:
        # Imported here so the fake engine and the tests need no GPU stack.
        from qwen_asr import Qwen3ASRModel

        self.name = model
        self._model = Qwen3ASRModel.LLM(
            model=model,
            gpu_memory_utilization=gpu_memory_utilization,
            max_new_tokens=32,
        )
        self._stream_args = dict(
            context=context,
            language=language,
            unfixed_chunk_num=unfixed_chunk_num,
            unfixed_token_num=unfixed_token_num,
            chunk_size_sec=chunk_size_sec,
        )

    def new_stream(self) -> Stream:
        return _QwenStream(self._model, self._model.init_streaming_state(**self._stream_args))


class _QwenStream:
    def __init__(self, model, state) -> None:
        self._model = model
        self._state = state

    def feed(self, pcm: np.ndarray) -> str:
        # The model buffers by itself and decodes whole chunks only.
        self._state = self._model.streaming_transcribe(pcm, self._state)
        return self._state.text

    def finish(self) -> str:
        self._state = self._model.finish_streaming_transcribe(self._state)
        return self._state.text
