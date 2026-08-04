"""Error handling patterns for Python.

Sentinel errors, error wrapping, error chains, context propagation.
"""

from __future__ import annotations

import traceback
from typing import TYPE_CHECKING, TypeVar

if TYPE_CHECKING:
    from typing import Callable


class ProjectNameError(Exception):
    """Base error — all project errors inherit from this."""

    code: str = "PROJECT_ERROR"

    def __init__(self, message: str, *, cause: Exception | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.cause = cause
        if cause:
            self.__cause__ = cause

    def __repr__(self) -> str:
        parts = [f"{self.__class__.__name__}({self.message!r}"]
        if self.cause:
            parts.append(f", cause={self.cause!r}")
        parts.append(")")
        return "".join(parts)

    def to_dict(self) -> dict:
        """Convert to dict for structured logging."""
        return {
            "error_type": self.code,
            "message": self.message,
            "cause": str(self.cause) if self.cause else None,
        }


class NotFoundError(ProjectNameError):
    """Raised when a requested resource does not exist."""

    code = "NOT_FOUND"

    def __init__(self, resource: str, identifier: str, *, cause: Exception | None = None) -> None:
        self.resource = resource
        self.identifier = identifier
        super().__init__(f"{resource} not found: {identifier}", cause=cause)


class ValidationError(ProjectNameError):
    """Raised when input validation fails."""

    code = "VALIDATION_ERROR"

    def __init__(self, message: str, *, cause: Exception | None = None) -> None:
        super().__init__(message, cause=cause)


class ConfigurationError(ProjectNameError):
    """Raised when configuration is invalid or missing."""

    code = "CONFIGURATION_ERROR"


class DatabaseError(ProjectNameError):
    """Raised when a database operation fails."""

    code = "DATABASE_ERROR"


E = TypeVar("E", bound=ProjectNameError)


def map_error(err: Exception, *, default: type[E] = ProjectNameError) -> E:
    """Map an internal error to a user-facing error without leaking details.

    Always log the real error; return only what the user should see.
    """
    import logging

    logger = logging.getLogger(__name__)
    logger.exception("internal error", exc_info=err)

    if isinstance(err, ProjectNameError):
        return err

    # Map common exceptions
    mapping: dict[type[Exception], type[E]] = {
        FileNotFoundError: NotFoundError,  # type: ignore[misc]
        ValueError: ValidationError,  # type: ignore[misc]
        TimeoutError: ConfigurationError,  # type: ignore[misc]
    }

    for exc_type, mapped_type in mapping.items():
        if isinstance(err, exc_type):
            return mapped_type(str(err), cause=err)

    return default(str(err), cause=err)


def reraise_with_context(
    err: Exception,
    message: str,
    *,
    raise_type: type[E] | None = None,
) -> "Callable[[], E]":
    """Re-raise an error with additional context.

    Usage:
        try:
            something()
        except Exception as e:
            raise reraise_with_context(e, "failed to do X")
    """

    def inner() -> "E":
        raise raise_type(message, cause=err) if raise_type else None  # type: ignore[arg-type]

    return inner


def trace_error(err: Exception) -> str:
    """Format an exception with full traceback for logging."""
    return "".join(traceback.format_exception(type(err), err, err.__traceback__))
