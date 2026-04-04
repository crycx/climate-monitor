import threading
import time
from collections import defaultdict, deque

from flask import request


_rate_limit_store: dict[str, deque[float]] = defaultdict(deque)
_rate_limit_lock = threading.Lock()


def remote_identity() -> str:
    forwarded_for = request.headers.get("X-Forwarded-For")
    if forwarded_for:
        # Trust the left-most value as client identity when behind a proxy.
        return forwarded_for.split(",")[0].strip()
    return request.remote_addr or "unknown"


def check_rate_limit(action: str, identity: str, limit_count: int, window_seconds: int) -> int:
    """Return 0 when allowed, or number of seconds until next allowed request."""
    now = time.time()
    window_start = now - window_seconds
    key = f"{action}:{identity}"

    with _rate_limit_lock:
        hits = _rate_limit_store[key]
        while hits and hits[0] < window_start:
            hits.popleft()

        if len(hits) >= limit_count:
            retry_after = int(hits[0] + window_seconds - now)
            return retry_after if retry_after > 0 else 1

        hits.append(now)
        return 0


def reset_rate_limit_store_for_tests() -> None:
    with _rate_limit_lock:
        _rate_limit_store.clear()
