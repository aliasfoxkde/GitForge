"""Configuration management using Pydantic Settings.

All configuration is environment-driven (12-factor app).
No hardcoded values.
"""

from __future__ import annotations

import os
from typing import Any

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class DatabaseSettings(BaseSettings):
    """Database configuration."""

    url: str = Field(default="sqlite:///./dev.db")
    pool_size: int = Field(default=5, ge=1, le=100)
    max_overflow: int = Field(default=10, ge=0, le=50)
    echo: bool = Field(default=False)

    model_config = SettingsConfigDict(env_prefix="DB_")


class HTTPSettings(BaseSettings):
    """HTTP server configuration."""

    host: str = Field(default="0.0.0.0")
    port: int = Field(default=8080, ge=1, le=65535)
    workers: int = Field(default=4, ge=1)
    reload: bool = Field(default=False)
    timeout_keep_alive: int = Field(default=30)

    model_config = SettingsConfigDict(env_prefix="HTTP_")


class LogSettings(BaseSettings):
    """Logging configuration."""

    level: str = Field(default="INFO")
    format: str = Field(default="json")  # "json" or "console"
    include_caller: bool = Field(default=True)
    include_timestamp: bool = Field(default=True)

    model_config = SettingsConfigDict(env_prefix="LOG_")


class AppSettings(BaseSettings):
    """Main application settings — aggregates all sub-settings."""

    env: str = Field(default="development")
    debug: bool = Field(default=False)
    app_name: str = Field(default="project-name")
    app_version: str = Field(default="0.1.0")

    db: DatabaseSettings = Field(default_factory=DatabaseSettings)
    http: HTTPSettings = Field(default_factory=HTTPSettings)
    log: LogSettings = Field(default_factory=LogSettings)

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        case_sensitive=False,
    )


def load_config() -> AppSettings:
    """Load and validate configuration from environment."""
    return AppSettings()


# Global config instance — lazily initialized
_config: AppSettings | None = None


def get_config() -> AppSettings:
    """Get the global config instance (singleton)."""
    global _config
    if _config is None:
        _config = load_config()
    return _config
