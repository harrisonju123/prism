from __future__ import annotations
from typing import TYPE_CHECKING, Any

from .exceptions import raise_for_status

if TYPE_CHECKING:
    from .client import PrismClient


class Keys:
    def __init__(self, client: "PrismClient"):
        self._client = client

    def create(
        self,
        name: str,
        *,
        rpm_limit: int | None = None,
        tpm_limit: int | None = None,
        daily_budget_usd: float | None = None,
        monthly_budget_usd: float | None = None,
        allowed_models: list[str] | None = None,
        rotation_interval_days: int | None = None,
        **kwargs: Any,
    ) -> dict:
        payload: dict = {"name": name, **kwargs}
        if rpm_limit is not None:
            payload["rpm_limit"] = rpm_limit
        if tpm_limit is not None:
            payload["tpm_limit"] = tpm_limit
        if daily_budget_usd is not None:
            payload["daily_budget_usd"] = daily_budget_usd
        if monthly_budget_usd is not None:
            payload["monthly_budget_usd"] = monthly_budget_usd
        if allowed_models is not None:
            payload["allowed_models"] = allowed_models
        if rotation_interval_days is not None:
            payload["rotation_interval_days"] = rotation_interval_days
        resp = self._client._http.post("/api/v1/keys", json=payload)
        data = resp.json() if resp.content else {}
        raise_for_status(resp.status_code, data)
        return data

    def list(self) -> list[dict]:
        resp = self._client._http.get("/api/v1/keys")
        data = resp.json() if resp.content else []
        raise_for_status(resp.status_code, data if isinstance(data, dict) else {})
        return data if isinstance(data, list) else data.get("keys", [])

    def revoke(self, key_id: str) -> None:
        resp = self._client._http.delete(f"/api/v1/keys/{key_id}")
        raise_for_status(resp.status_code, {})

    def rotate(self, key_id: str) -> dict:
        resp = self._client._http.post(f"/api/v1/keys/{key_id}/rotate")
        data = resp.json() if resp.content else {}
        raise_for_status(resp.status_code, data)
        return data

    def usage(self, key_id: str) -> dict:
        resp = self._client._http.get(f"/api/v1/keys/{key_id}/usage")
        data = resp.json() if resp.content else {}
        raise_for_status(resp.status_code, data)
        return data
