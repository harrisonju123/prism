from __future__ import annotations
from typing import TYPE_CHECKING, Any

from .exceptions import raise_for_status

if TYPE_CHECKING:
    from .client import PrismClient


class EmbeddingResponse:
    def __init__(self, data: dict):
        self._data = data
        self.model = data.get("model", "")
        items = data.get("data", [])
        self.embeddings = [item.get("embedding", []) for item in items]
        self.usage = data.get("usage", {})


class Embeddings:
    def __init__(self, client: "PrismClient"):
        self._client = client

    def create(self, model: str, input: str | list[str], **kwargs: Any) -> EmbeddingResponse:
        payload = {"model": model, "input": input, **kwargs}
        resp = self._client._http.post("/v1/embeddings", json=payload)
        data = resp.json() if resp.content else {}
        raise_for_status(resp.status_code, data)
        return EmbeddingResponse(data)

    async def acreate(self, model: str, input: str | list[str], **kwargs: Any) -> EmbeddingResponse:
        payload = {"model": model, "input": input, **kwargs}
        resp = await self._client._async_http.post("/v1/embeddings", json=payload)
        data = resp.json() if resp.content else {}
        raise_for_status(resp.status_code, data)
        return EmbeddingResponse(data)
