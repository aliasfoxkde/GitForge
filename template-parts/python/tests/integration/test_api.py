"""Integration tests for API layer.

Integration tests verify that components work together correctly.
Use a test database, real HTTP client, etc.
Mark: pytest.mark.integration
"""

from __future__ import annotations

import pytest

pytestmark = pytest.mark.integration


class TestAPI:
    """Integration tests for API endpoints."""

    def test_health_check(self) -> None:
        """Health endpoint returns 200."""
        # TODO: Use httpx + test server
        assert True  # Placeholder

    def test_create_resource(self) -> None:
        """POST /resources creates a new resource."""
        assert True  # Placeholder
