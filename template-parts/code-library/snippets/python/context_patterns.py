"""Context propagation patterns for Python.

Context managers, cancellation, timeout, resource cleanup.
"""

from __future__ import annotations

import asyncio
import contextlib
import time
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any, AsyncIterator, Iterator

if TYPE_CHECKING:
    pass


@contextmanager
def timeout_context(seconds: float) -> Iterator[None]:
    """Context manager that raises TimeoutError after N seconds.

    Usage:
        with timeout_context(5):
            long_operation()
    """
    start = time.monotonic()
    yield
    elapsed = time.monotonic() - start
    if elapsed > seconds:
        raise TimeoutError(f"operation took {elapsed:.2f}s, limit was {seconds}s")


@contextmanager
def timer_context(label: str = "operation") -> Iterator[None]:
    """Context manager that logs execution time.

    Usage:
        with timer_context("data_load"):
            load_data()
    """
    import logging

    logger = logging.getLogger(__name__)
    start = time.monotonic()
    try:
        yield
    finally:
        elapsed = time.monotonic() - start
        logger.info("%s took %.3fs", label, elapsed)


@contextmanager
def suppress_errors(*error_types: type[Exception]) -> Iterator[None]:
    """Context manager that suppresses specified exception types.

    Usage:
        with suppress_errors(FileNotFoundError, PermissionError):
            os.remove(path)
    """
    try:
        yield
    except error_types:
        pass


class ResourceContext:
    """Base class for resource management with context manager support."""

    def __init__(self, resource_id: str) -> None:
        self.resource_id = resource_id
        self._open = False

    def __enter__(self) -> "ResourceContext":
        self._open = True
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> bool:
        self._open = False
        self.close()
        return False  # Don't suppress exceptions

    def close(self) -> None:
        """Override in subclass to clean up resources."""
        pass


async def async_timeout(coro: Any, seconds: float) -> Any:
    """Run a coroutine with a timeout.

    Usage:
        result = await async_timeout(do_something(), 5)
    """
    try:
        return await asyncio.wait_for(coro, timeout=seconds)
    except asyncio.TimeoutError:
        raise TimeoutError(f"coroutine timed out after {seconds}s")


@asynccontextmanager
async def async_resource() -> AsyncIterator[None]:
    """Example async context manager for resource cleanup.

    Usage:
        async with async_resource():
            await do_something()
    """
    try:
        yield
    finally:
        await asyncio.sleep(0)  # Allow other coroutines to run


def run_with_timeout(func: Any, timeout: float, *args: Any, **kwargs: Any) -> Any:
    """Run a blocking function with a timeout using a thread.

    Usage:
        result = run_with_timeout(blocking_io, 5, arg1, arg2)
    """
    import concurrent.futures

    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
        future = executor.submit(func, *args, **kwargs)
        return future.result(timeout=timeout)
