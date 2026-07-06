"""API routes for {{PROJECT_NAME}}."""

from fastapi import APIRouter, Depends, HTTPException, status
from typing import Annotated

router = APIRouter(prefix="/api/v1", tags=["api"])


@router.get("/health")
async def health_check() -> dict[str, str]:
    """Health check endpoint."""
    return {"status": "healthy"}


@router.get("/items/{item_id}")
async def get_item(item_id: int) -> dict:
    """Get item by ID."""
    if item_id <= 0:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Invalid item ID")
    return {"id": item_id, "name": f"item_{item_id}"}


@router.post("/items/")
async def create_item(item: dict) -> dict:
    """Create a new item."""
    return {"id": 1, **item}
