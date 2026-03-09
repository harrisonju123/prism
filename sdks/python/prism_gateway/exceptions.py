class PrismError(Exception):
    def __init__(self, message: str, status_code: int | None = None, response: dict | None = None):
        super().__init__(message)
        self.status_code = status_code
        self.response = response or {}

class BudgetExceededError(PrismError):
    pass

class RateLimitError(PrismError):
    pass

class CircuitOpenError(PrismError):
    pass

def raise_for_status(status_code: int, response: dict) -> None:
    msg = response.get("error", {}).get("message", "unknown error") if isinstance(response, dict) else str(response)
    if status_code == 429:
        raise RateLimitError(msg, status_code, response)
    elif status_code == 402:
        raise BudgetExceededError(msg, status_code, response)
    elif status_code == 503:
        raise CircuitOpenError(msg, status_code, response)
    elif status_code >= 400:
        raise PrismError(msg, status_code, response)
