"""Retry patterns with exponential backoff and jitter.

Usage:
    result = await retry(do_something, max_attempts=3, base_delay=1.0)
"""

from __future__ import annotations

import asyncio
import random
import time
from typing import TYPE_CHECKING, Any, Awaitable, Callable, TypeVar

if TYPE_CHECKING:
    pass

T = TypeVar("T")


def retry(
    func: Callable[[], T],
    *,
    max_attempts: int = 3,
    base_delay: float = 1.0,
    max_delay: float = 60.0,
    jitter: bool = True,
    exceptions: tuple[type[Exception], ...] = (Exception,),
) -> T:
    """Synchronous retry with exponential backoff.

    Args:
        func: Function to retry
        max_attempts: Maximum number of attempts
        base_delay: Base delay between retries (seconds)
        max_delay: Maximum delay (seconds)
        jitter: Add random ±25% jitter to delay
        exceptions: Tuple of exceptions to catch and retry

    Returns:
        Return value of func on success

    Raises:
        Last exception if all attempts fail
    """
    last_exc: Exception | None = None

    for attempt in range(1, max_attempts + 1):
        try:
            return func()
        except exceptions as e:
            last_exc = e
            if attempt >= max_attempts:
                break

            delay = min(base_delay * (2 ** (attempt - 1)), max_delay)
            if jitter:
                delay = delay * (0.75 + random.random() * 0.5)

            time.sleep(delay)

    if last_exc is not None:
        raise last_exc
    raise RuntimeError("retry logic error")


async def retry_async(
    coro: Callable[[], Awaitable[T]],
    *,
    max_attempts: int = 3,
    base_delay: float = 1.0,
    max_delay: float = 60.0,
    jitter: bool = True,
    exceptions: tuple[type[Exception], ...] = (Exception,),
) -> T:
    """Async retry with exponential backoff.

    Usage:
        result = await retry_async(do_something_async, max_attempts=3)
    """
    last_exc: Exception | None = None

    for attempt in range(1, max_attempts + 1):
        try:
            return await coro()
        except exceptions as e:
            last_exc = e
            if attempt >= max_attempts:
                break

            delay = min(base_delay * (2 ** (attempt - 1)), max_delay)
            if jitter:
                delay = delay * (0.75 + random.random() * 0.5)

            await asyncio.sleep(delay)

    if last_exc is not None:
        raise last_exc
    raise RuntimeError("retry_async logic error")
