"""Batch processing patterns for Python.

Chunked processing, progress tracking, error handling for large datasets.

Usage:
    processor = BatchProcessor(
        chunk_size=100,
        max_workers=4,
        on_progress=progress_callback,
    )
    results = processor.process(process_item, items)
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import threading
from dataclasses import dataclass, field
from typing import Any, Callable, Generic, Iterator, TypeVar

T = TypeVar("T")
R = TypeVar("R")


@dataclass
class BatchResult(Generic[T]):
    """Result of processing a batch of items."""

    successful: list[R] = field(default_factory=list)
    failed: list[tuple[T, Exception]] = field(default_factory=list)
    total_processed: int = 0

    @property
    def success_count(self) -> int:
        return len(self.successful)

    @property
    def failure_count(self) -> int:
        return len(self.failed)

    @property
    def all_succeeded(self) -> bool:
        return self.failure_count == 0


class BatchProcessor(Generic[T, R]):
    """Processes items in batches with parallel execution.

    Args:
        chunk_size: Number of items per batch.
        max_workers: Maximum parallel workers.
        on_progress: Optional callback(processed, total).
    """

    def __init__(
        self,
        chunk_size: int = 100,
        max_workers: int = 4,
        on_progress: Callable[[int, int], None] | None = None,
    ) -> None:
        if chunk_size <= 0:
            raise ValueError("chunk_size must be positive")
        if max_workers <= 0:
            raise ValueError("max_workers must be positive")

        self.chunk_size = chunk_size
        self.max_workers = max_workers
        self.on_progress = on_progress

    def process(
        self,
        func: Callable[[T], R],
        items: list[T],
    ) -> BatchResult[T, R]:
        """Process items synchronously.

        Args:
            func: Function to apply to each item.
            items: Items to process.

        Returns:
            BatchResult with successful and failed items.
        """
        result = BatchResult[T, R]()
        total = len(items)
        processed = 0

        for chunk in self._chunks(items):
            chunk_results = self._process_chunk_sync(func, chunk, result)
            processed += len(chunk_results)
            if self.on_progress:
                self.on_progress(processed, total)

        result.total_processed = processed
        return result

    def _process_chunk_sync(
        self,
        func: Callable[[T], R],
        chunk: list[T],
        result: BatchResult[T, R],
    ) -> list[R]:
        """Process a single chunk with thread pool."""
        with concurrent.futures.ThreadPoolExecutor(max_workers=self.max_workers) as executor:
            future_to_item = {
                executor.submit(self._safe_process, func, item): item
                for item in chunk
            }

            chunk_results = []
            for future in concurrent.futures.as_completed(future_to_item):
                item = future_to_item[future]
                try:
                    chunk_results.append(future.result())
                except Exception as e:
                    result.failed.append((item, e))
            return chunk_results

    @staticmethod
    def _safe_process(func: Callable[[T], R], item: T) -> R:
        """Execute func with error wrapping."""
        return func(item)

    def _chunks(self, items: list[T]) -> Iterator[list[T]]:
        """Yield successive chunks from items."""
        for i in range(0, len(items), self.chunk_size):
            yield items[i : i + self.chunk_size]


class AsyncBatchProcessor(Generic[T, R]):
    """Async batch processor with parallel execution.

    Args:
        chunk_size: Number of items per batch.
        max_concurrent: Maximum concurrent batches.
        on_progress: Optional async callback.
    """

    def __init__(
        self,
        chunk_size: int = 100,
        max_concurrent: int = 4,
        on_progress: Callable[[int, int], Any] | None = None,
    ) -> None:
        self.chunk_size = chunk_size
        self.max_concurrent = max_concurrent
        self.on_progress = on_progress

    async def process(
        self,
        func: Callable[[T], Any],
        items: list[T],
    ) -> BatchResult[T, R]:
        """Process items asynchronously.

        Args:
            func: Async function to apply to each item.
            items: Items to process.

        Returns:
            BatchResult with successful and failed items.
        """
        result = BatchResult[T, R]()
        total = len(items)
        processed = 0
        semaphore = asyncio.Semaphore(self.max_concurrent)

        async def process_chunk(chunk: list[T]) -> None:
            nonlocal processed

            async with semaphore:
                tasks = [self._safe_process_async(func, item) for item in chunk]
                chunk_results = await asyncio.gather(*tasks, return_exceptions=True)

                for item, res in zip(chunk, chunk_results):
                    if isinstance(res, Exception):
                        result.failed.append((item, res))
                    else:
                        result.successful.append(res)

                processed += len(chunk)
                if self.on_progress:
                    await asyncio.coroutine(self.on_progress)(processed, total)

        # Process chunks with concurrency limit
        await asyncio.gather(*[process_chunk(c) for c in self._chunks(items)])
        result.total_processed = len(items)
        return result

    async def _safe_process_async(
        self,
        func: Callable[[T], Any],
        item: T,
    ) -> R:
        """Execute async func with error wrapping."""
        return await func(item)

    def _chunks(self, items: list[T]) -> Iterator[list[T]]:
        """Yield successive chunks from items."""
        for i in range(0, len(items), self.chunk_size):
            yield items[i : i + self.chunk_size]


def chunked_iterator(
    items: Iterator[T],
    chunk_size: int,
) -> Iterator[list[T]]:
    """Yield chunks from an iterator.

    Usage:
        for chunk in chunked_iterator(large_dataset(), 100):
            process(chunk)
    """
    chunk: list[T] = []
    for item in items:
        chunk.append(item)
        if len(chunk) >= chunk_size:
            yield chunk
            chunk = []
    if chunk:
        yield chunk


@dataclass
class ProgressTracker:
    """Simple progress tracking with threading support."""

    total: int
    description: str = "Processing"
    _lock: threading.Lock = field(default_factory=threading.Lock)
    _processed: int = field(default=0, init=False)

    @property
    def processed(self) -> int:
        with self._lock:
            return self._processed

    def increment(self, count: int = 1) -> None:
        with self._lock:
            self._processed += count
            self._log_progress()

    def _log_progress(self) -> None:
        pct = (self._processed / self.total) * 100 if self.total > 0 else 0
        print(f"\r{self.description}: {self._processed}/{self.total} ({pct:.1f}%)", end="", flush=True)

    def finish(self) -> None:
        print()  # Newline after progress
