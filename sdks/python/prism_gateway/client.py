import os
import httpx
from .chat import ChatCompletions
from .embeddings import Embeddings
from .keys import Keys


class _ChatNamespace:
    def __init__(self, client: "PrismClient"):
        self.completions = ChatCompletions(client)


class PrismClient:
    """OpenAI-compatible client for the PrisM LLM Gateway.

    Usage:
        client = PrismClient(base_url="http://localhost:9100", api_key="prism_...")
        response = client.chat.completions.create(
            model="smart",
            messages=[{"role": "user", "content": "Hello!"}]
        )
        print(response.content)
    """

    def __init__(
        self,
        base_url: str | None = None,
        api_key: str | None = None,
        timeout: float = 60.0,
    ):
        self.base_url = (base_url or os.environ.get("PRISM_URL", "http://localhost:9100")).rstrip("/")
        self.api_key = api_key or os.environ.get("PRISM_API_KEY", "")

        headers = {"User-Agent": "prism-gateway-python/0.1"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        self._http = httpx.Client(
            base_url=self.base_url,
            headers=headers,
            timeout=timeout,
        )
        self._async_http = httpx.AsyncClient(
            base_url=self.base_url,
            headers=headers,
            timeout=timeout,
        )

        self.chat = _ChatNamespace(self)
        self.embeddings = Embeddings(self)
        self.keys = Keys(self)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self._http.close()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        await self._async_http.aclose()

    def health(self) -> dict:
        resp = self._http.get("/health")
        return resp.json()

    def models(self) -> dict:
        resp = self._http.get("/v1/models")
        return resp.json()
