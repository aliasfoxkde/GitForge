"""Middleware for {{PROJECT_NAME}}."""

from fastapi import Request, Response
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.types import ASGIApp
import structlog

logger = structlog.get_logger()


class LoggingMiddleware(BaseHTTPMiddleware):
    """Middleware for structured request/response logging."""

    async def dispatch(self, request: Request, call_next: ASGIApp) -> Response:
        """Log incoming requests and outgoing responses."""
        response = await call_next(request)
        return response


def setup_cors(app: ASGIApp) -> ASGIApp:
    """Configure CORS settings."""
    from fastapi.middleware.cors import CORSMiddleware

    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    return app
