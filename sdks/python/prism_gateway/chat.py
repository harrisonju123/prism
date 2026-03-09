from __future__ import annotations
import json
import time
from typing import TYPE_CHECKING, Iterator, AsyncIterator, Any

import httpx

from .exceptions import raise_for_status

if TYPE_CHECKING:
    from .client import PrismClient


class ChatCompletionChunk:
    def __init__(self, data: dict):
        self._data = data
        self.id = data.get("id", "")
        self.model = data.get("model", "")
        choices = data.get("choices", [{}])
        choice = choices[0] if choices else {}
        delta = choice.get("delta", {})
        self.delta_content = delta.get("content") or ""
        self.finish_reason = choice.get("finish_reason")


class ChatCompletion:
    def __init__(self, data: dict):
        self._data = data
        self.id = data.get("id", "")
        self.model = data.get("model", "")
        choices = data.get("choices", [{}])
        choice = choices[0] if choices else {}
        self.message = choice.get("message", {})
        self.content = self.message.get("content", "")
        self.finish_reason = choice.get("finish_reason")
        usage = data.get("usage", {})
        self.usage = usage

    def __repr__(self) -> str:
        return f"ChatCompletion(model={self.model!r}, content={self.content[:80]!r})"


MAX_RETRIES = 3
RETRY_DELAY = 1.0


class ChatCompletions:
    def __init__(self, client: "PrismClient"):
        self._client = client

    def create(
        self,
        model: str,
        messages: list[dict],
        *,
        stream: bool = False,
        **kwargs: Any,
    ) -> ChatCompletion | Iterator[ChatCompletionChunk]:
        payload = {"model": model, "messages": messages, "stream": stream, **kwargs}
        if stream:
            return self._stream_sync(payload)
        return self._send_sync(payload)

    async def acreate(
        self,
        model: str,
        messages: list[dict],
        *,
        stream: bool = False,
        **kwargs: Any,
    ) -> ChatCompletion | AsyncIterator[ChatCompletionChunk]:
        payload = {"model": model, "messages": messages, "stream": stream, **kwargs}
        if stream:
            return self._stream_async(payload)
        return await self._send_async(payload)

    def _send_sync(self, payload: dict) -> ChatCompletion:
        for attempt in range(MAX_RETRIES):
            resp = self._client._http.post("/v1/chat/completions", json=payload)
            if resp.status_code == 429 and attempt < MAX_RETRIES - 1:
                time.sleep(RETRY_DELAY * (2 ** attempt))
                continue
            data = resp.json() if resp.content else {}
            raise_for_status(resp.status_code, data)
            return ChatCompletion(data)
        raise RuntimeError("unreachable")

    async def _send_async(self, payload: dict) -> ChatCompletion:
        import asyncio
        for attempt in range(MAX_RETRIES):
            resp = await self._client._async_http.post("/v1/chat/completions", json=payload)
            if resp.status_code == 429 and attempt < MAX_RETRIES - 1:
                await asyncio.sleep(RETRY_DELAY * (2 ** attempt))
                continue
            data = resp.json() if resp.content else {}
            raise_for_status(resp.status_code, data)
            return ChatCompletion(data)
        raise RuntimeError("unreachable")

    def _stream_sync(self, payload: dict) -> Iterator[ChatCompletionChunk]:
        with self._client._http.stream("POST", "/v1/chat/completions", json=payload) as resp:
            raise_for_status(resp.status_code, {})
            for line in resp.iter_lines():
                if line.startswith("data: "):
                    data_str = line[6:]
                    if data_str.strip() == "[DONE]":
                        break
                    try:
                        data = json.loads(data_str)
                        yield ChatCompletionChunk(data)
                    except json.JSONDecodeError:
                        pass

    async def _stream_async(self, payload: dict) -> AsyncIterator[ChatCompletionChunk]:
        async with self._client._async_http.stream("POST", "/v1/chat/completions", json=payload) as resp:
            raise_for_status(resp.status_code, {})
            async for line in resp.aiter_lines():
                if line.startswith("data: "):
                    data_str = line[6:]
                    if data_str.strip() == "[DONE]":
                        break
                    try:
                        data = json.loads(data_str)
                        yield ChatCompletionChunk(data)
                    except json.JSONDecodeError:
                        pass
