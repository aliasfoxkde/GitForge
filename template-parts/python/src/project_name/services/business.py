"""Business logic layer for {{PROJECT_NAME}}."""

from typing import Optional, List
from datetime import datetime
import structlog

logger = structlog.get_logger()


class BusinessError(Exception):
    """Base exception for business logic errors."""

    def __init__(self, message: str, code: str = "BUSINESS_ERROR"):
        self.message = message
        self.code = code
        super().__init__(message)


class ItemNotFoundError(BusinessError):
    """Raised when an item cannot be found."""

    def __init__(self, item_id: int):
        super().__init__(f"Item {item_id} not found", code="ITEM_NOT_FOUND")
        self.item_id = item_id


class ItemService:
    """Service for managing items."""

    def __init__(self) -> None:
        """Initialize the item service."""
        self._items: dict[int, dict] = {}
        self._counter = 0

    async def create_item(
        self,
        name: str,
        description: Optional[str] = None,
        quantity: int = 0,
    ) -> dict:
        """Create a new item."""
        self._counter += 1
        item = {
            "id": self._counter,
            "name": name,
            "description": description,
            "quantity": quantity,
            "created_at": datetime.utcnow().isoformat(),
            "updated_at": datetime.utcnow().isoformat(),
        }
        self._items[self._counter] = item
        logger.info("item_created", item_id=self._counter, name=name)
        return item

    async def get_item(self, item_id: int) -> dict:
        """Retrieve an item by ID."""
        if item_id not in self._items:
            raise ItemNotFoundError(item_id)
        return self._items[item_id]

    async def list_items(self) -> List[dict]:
        """List all items."""
        return list(self._items.values())
