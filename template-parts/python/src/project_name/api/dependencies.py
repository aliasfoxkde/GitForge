"""FastAPI dependencies for {{PROJECT_NAME}}."""

from typing import Annotated, Optional
from fastapi import Depends, Header, HTTPException, status
from pydantic import BaseModel


class CurrentUser(BaseModel):
    """Represents the current authenticated user."""

    id: int
    username: str
    email: Optional[str] = None


async def get_current_user(
    authorization: Annotated[Optional[str], Header()] = None
) -> CurrentUser:
    """Extract and validate current user from request."""
    if not authorization:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Missing authorization header",
        )
    return CurrentUser(id=1, username="placeholder")


async def require_admin(
    user: CurrentUser = Depends(get_current_user),
) -> CurrentUser:
    """Require admin privileges for this endpoint."""
    if user.username != "admin":
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin privileges required",
        )
    return user
