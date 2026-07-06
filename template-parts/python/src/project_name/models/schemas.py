"""Pydantic schemas for {{PROJECT_NAME}}."""

from pydantic import BaseModel, Field, field_validator
from typing import Optional
from datetime import datetime


class ItemBase(BaseModel):
    """Base item schema with common fields."""

    name: str = Field(..., min_length=1, max_length=255)
    description: Optional[str] = Field(None, max_length=1000)
    quantity: int = Field(default=0, ge=0)


class ItemCreate(ItemBase):
    """Schema for creating a new item."""

    @field_validator("name")
    @classmethod
    def name_not_empty(cls, v: str) -> str:
        if not v.strip():
            raise ValueError("Name cannot be empty or whitespace")
        return v.strip()


class ItemUpdate(BaseModel):
    """Schema for updating an existing item."""

    name: Optional[str] = Field(None, min_length=1, max_length=255)
    description: Optional[str] = Field(None, max_length=1000)
    quantity: Optional[int] = Field(None, ge=0)


class Item(ItemBase):
    """Schema for a complete item with all fields."""

    id: int
    created_at: datetime
    updated_at: datetime

    class Config:
        from_attributes = True


class ErrorResponse(BaseModel):
    """Standard error response schema."""

    error: str
    detail: Optional[str] = None
    timestamp: datetime = Field(default_factory=datetime.utcnow)
