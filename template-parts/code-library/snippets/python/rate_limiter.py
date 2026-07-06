"""Token bucket rate limiter for Python.

Controls the rate of operations to prevent overwhelming downstream services.

Usage:
    limiter = TokenBucketRateLimiter(rate=100, capacity=100)  # 100 per second
    await limiter.acquire()
    await do_work()

    # Or sync version
    limiter = SyncTokenBucketRateLimiter(rate=10, capacity=10)
    limiter.acquire()
    do_work()
"""

from __future__ import annotations

import asyncio
import threading
import time
from typing import Final


class TokenBucketRateLimiter:
    """Async token bucket rate limiter.

    Args:
        rate: Number of tokens added per second.
        capacity: Maximum number of tokens in the bucket.
    """

    def __init__(self, rate: float, capacity: int) -> None:
        if rate <= 0:
            raise ValueError("rate must be positive")
        if capacity <= 0:
            raise ValueError("capacity must be positive")

        self._rate: Final[float] = rate
        self._capacity: Final[int] = capacity
        self._tokens: float = float(capacity)
        self._last_update: float = time.monotonic()
        self._lock: asyncio.Lock = asyncio.Lock()

    async def acquire(self, tokens: int = 1) -> None:
        """Acquire tokens, waiting if necessary.

        Args:
            tokens: Number of tokens to acquire.
        """
        await self._wait_for_tokens(tokens)

    async def _wait_for_tokens(self, tokens: int) -> None:
        async with self._lock:
            await self._refill()

            if self._tokens >= tokens:
                self._tokens -= tokens
                return

            # Calculate wait time
            needed = tokens - self._tokens
            wait_time = needed / self._rate

        await asyncio.sleep(wait_time)

        async with self._lock:
            await self._refill()
            self._tokens -= tokens

    async def _refill(self) -> None:
        """Refill tokens based on elapsed time."""
        now = time.monotonic()
        elapsed = now - self._last_update
        self._last_update = now

        self._tokens += elapsed * self._rate
        if self._tokens > self._capacity:
            self._tokens = float(self._capacity)

    async def try_acquire(self, tokens: int = 1) -> bool:
        """Try to acquire tokens without blocking.

        Returns:
            True if tokens were acquired, False otherwise.
        """
        async with self._lock:
            await self._refill()

            if self._tokens >= tokens:
                self._tokens -= tokens
                return True
            return False

    @property
    def tokens(self) -> float:
        """Current number of available tokens."""
        return self._tokens

    @property
    def capacity(self) -> int:
        """Maximum bucket capacity."""
        return self._capacity

    @property
    def rate(self) -> float:
        """Token generation rate per second."""
        return self._rate


class SyncTokenBucketRateLimiter:
    """Synchronous token bucket rate limiter.

    Args:
        rate: Number of tokens added per second.
        capacity: Maximum number of tokens in the bucket.
    """

    def __init__(self, rate: float, capacity: int) -> None:
        if rate <= 0:
            raise ValueError("rate must be positive")
        if capacity <= 0:
            raise ValueError("capacity must be positive")

        self._rate = rate
        self._capacity = capacity
        self._tokens = float(capacity)
        self._last_update = time.monotonic()
        self._lock = threading.Lock()

    def acquire(self, tokens: int = 1) -> None:
        """Acquire tokens, blocking until available.

        Args:
            tokens: Number of tokens to acquire.
        """
        with self._lock:
            self._refill_locked()

            if self._tokens >= tokens:
                self._tokens -= tokens
                return

            # Calculate wait time
            needed = tokens - self._tokens
            wait_time = needed / self._rate

        time.sleep(wait_time)

        with self._lock:
            self._refill_locked()
            self._tokens -= tokens

    def _refill_locked(self) -> None:
        """Refill tokens based on elapsed time. Must be called with lock held."""
        now = time.monotonic()
        elapsed = now - self._last_update
        self._last_update = now

        self._tokens += elapsed * self._rate
        if self._tokens > self._capacity:
            self._tokens = float(self._capacity)

    def try_acquire(self, tokens: int = 1) -> bool:
        """Try to acquire tokens without blocking.

        Returns:
            True if tokens were acquired, False otherwise.
        """
        with self._lock:
            self._refill_locked()

            if self._tokens >= tokens:
                self._tokens -= tokens
                return True
            return False

    @property
    def tokens(self) -> float:
        """Current number of available tokens."""
        with self._lock:
            self._refill_locked()
            return self._tokens

    def reset(self) -> None:
        """Reset the limiter to full capacity."""
        with self._lock:
            self._tokens = float(self._capacity)
            self._last_update = time.monotonic()


class SlidingWindowRateLimiter:
    """Sliding window rate limiter.

    Tracks requests in a sliding time window for more precise rate limiting.

    Args:
        rate: Maximum number of requests allowed in the window.
        window_seconds: Size of the sliding window in seconds.
    """

    def __init__(self, rate: int, window_seconds: float) -> None:
        self._rate = rate
        self._window_seconds = window_seconds
        self._requests: list[float] = []
        self._lock = threading.Lock()

    async def acquire(self) -> None:
        """Acquire a slot, waiting if the rate limit is exceeded."""
        while True:
            if await self._try_acquire_async():
                return
            await asyncio.sleep(0.01)

    def acquire_sync(self) -> None:
        """Synchronous acquire."""
        while True:
            if self._try_acquire_sync():
                return
            time.sleep(0.01)

    async def _try_acquire_async(self) -> bool:
        async with self._lock:
            return self._try_acquire_locked()

    def _try_acquire_sync(self) -> bool:
        with self._lock:
            return self._try_acquire_locked()

    def _try_acquire_locked(self) -> bool:
        now = time.monotonic()
        cutoff = now - self._window_seconds

        # Remove expired requests
        self._requests = [t for t in self._requests if t > cutoff]

        if len(self._requests) < self._rate:
            self._requests.append(now)
            return True
        return False
