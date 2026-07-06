"""Pytest configuration and shared fixtures.

All test fixtures live here — import from `tests` package.
Use `pytest.mark.unit`, `pytest.mark.integration`, `pytest.mark.e2e` to select.
"""

from __future__ import annotations

import os
from typing import Generator

import pytest
from pydantic import Field
from pydantic_settings import BaseSettings

from project_name.config import AppSettings, DatabaseSettings, HTTPSettings, LogSettings


@pytest.fixture
def test_config() -> AppSettings:
    """Override config for tests — isolated database, verbose logging."""
    return AppSettings(
        env="test",
        debug=True,
        db=DatabaseSettings(url="sqlite:///:memory:", echo=False),
        http=HTTPSettings(port=0),
        log=LogSettings(level="DEBUG", format="console"),
    )


@pytest.fixture
def sample_data() -> dict[str, str]:
    """Sample data for tests — no real PII, no production-like secrets."""
    return {
        "name": "Test Item",
        "email": "test@example.com",
        "description": "A test item for unit testing",
    }
