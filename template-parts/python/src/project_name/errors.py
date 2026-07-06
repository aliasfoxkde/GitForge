"""Sentinel errors for the project.

Define package-level sentinel errors. Use class-based errors for
rich context; use simple Exception subclasses for simple cases.
"""

from __future__ import annotations


class ProjectNameError(Exception):
    """Base error for all project-specific errors."""

    pass


class NotFoundError(ProjectNameError):
    """Raised when a requested resource does not exist."""

    def __init__(self, resource: str, identifier: str) -> None:
        self.resource = resource
        self.identifier = identifier
        super().__init__(f"{resource} not found: {identifier}")


class ValidationError(ProjectNameError):
    """Raised when input validation fails."""

    pass


class ConfigurationError(ProjectNameError):
    """Raised when configuration is invalid or missing."""

    pass


class DatabaseError(ProjectNameError):
    """Raised when a database operation fails."""

    pass
