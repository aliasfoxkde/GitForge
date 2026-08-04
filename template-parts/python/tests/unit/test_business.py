"""Unit tests for business logic layer.

Unit tests are fast, isolated, and test one function/method at a time.
Mark: pytest.mark.unit
"""

from __future__ import annotations

import pytest

from project_name.errors import NotFoundError, ValidationError

pytestmark = pytest.mark.unit


class TestBusinessLogic:
    """Tests for core business logic."""

    def test_calculate_returns_correct_result(self) -> None:
        """Basic arithmetic — verify expected results."""
        result = 1 + 1
        assert result == 2

    def test_validation_error_on_empty_input(self) -> None:
        """Empty input should raise ValidationError."""
        with pytest.raises(ValidationError):
            pass  # TODO: call the actual function

    def test_not_found_error_for_missing_resource(self) -> None:
        """Missing resource should raise NotFoundError with context."""
        with pytest.raises(NotFoundError) as exc_info:
            raise NotFoundError("User", "nonexistent-id")
        assert "User" in str(exc_info.value)
        assert "nonexistent-id" in str(exc_info.value)
