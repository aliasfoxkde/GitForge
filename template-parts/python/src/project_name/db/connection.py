"""Database connection management for {{PROJECT_NAME}}."""

from contextlib import asynccontextmanager
from typing import AsyncGenerator, Optional
import structlog

logger = structlog.get_logger()


class DatabaseConfig:
    """Database configuration."""

    def __init__(
        self,
        host: str = "localhost",
        port: int = 5432,
        database: str = "{{PROJECT_NAME}}",
        user: str = "postgres",
        password: str = "",
        pool_min: int = 2,
        pool_max: int = 10,
    ):
        self.host = host
        self.port = port
        self.database = database
        self.user = user
        self.password = password
        self.pool_min = pool_min
        self.pool_max = pool_max


class DatabaseConnection:
    """Async database connection manager."""

    def __init__(self, config: Optional[DatabaseConfig] = None):
        """Initialize database connection manager."""
        self.config = config or DatabaseConfig()
        self._pool: Optional[object] = None

    async def connect(self) -> None:
        """Establish database connection pool."""
        logger.info(
            "db_connecting",
            host=self.config.host,
            port=self.config.port,
            database=self.config.database,
        )

    async def disconnect(self) -> None:
        """Close database connection pool."""
        if self._pool:
            await self._pool.close()
            logger.info("db_disconnected")

    @asynccontextmanager
    async def session(self) -> AsyncGenerator:
        """Context manager for database sessions."""
        try:
            yield {}
        finally:
            pass


_db: Optional[DatabaseConnection] = None


def get_database() -> DatabaseConnection:
    """Get the global database instance."""
    global _db
    if _db is None:
        _db = DatabaseConnection()
    return _db


async def init_database() -> None:
    """Initialize database connection on application startup."""
    db = get_database()
    await db.connect()


async def close_database() -> None:
    """Close database connection on application shutdown."""
    db = get_database()
    await db.disconnect()
