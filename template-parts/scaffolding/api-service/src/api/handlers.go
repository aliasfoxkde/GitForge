package api

import (
	"net/http"
	"strconv"

	"github.com/example/api-service/src/models"
	"github.com/example/api-service/src/services"
	"github.com/gin-gonic/gin"
)

// HealthHandler returns service health status.
func HealthHandler(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{
		"status":  "healthy",
		"service": "api-service",
	})
}

// ListItemsHandler returns all items.
func ListItemsHandler(c *gin.Context) {
	items, err := services.ListItems()
	if err != nil {
		c.JSON(http.StatusInternalServerError, models.ErrorResponse{Error: err.Error()})
		return
	}
	c.JSON(http.StatusOK, models.ListItemsResponse{Items: items})
}

// GetItemHandler returns a single item by ID.
func GetItemHandler(c *gin.Context) {
	idStr := c.Param("id")
	id, err := strconv.ParseUint(idStr, 10, 64)
	if err != nil {
		c.JSON(http.StatusBadRequest, models.ErrorResponse{Error: "invalid item ID"})
		return
	}

	item, err := services.GetItem(id)
	if err != nil {
		c.JSON(http.StatusNotFound, models.ErrorResponse{Error: "item not found"})
		return
	}
	c.JSON(http.StatusOK, item)
}

// CreateItemHandler creates a new item.
func CreateItemHandler(c *gin.Context) {
	var req models.CreateItemRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, models.ErrorResponse{Error: err.Error()})
		return
	}

	item, err := services.CreateItem(&req)
	if err != nil {
		c.JSON(http.StatusInternalServerError, models.ErrorResponse{Error: err.Error()})
		return
	}
	c.JSON(http.StatusCreated, item)
}

// UpdateItemHandler updates an existing item.
func UpdateItemHandler(c *gin.Context) {
	idStr := c.Param("id")
	id, err := strconv.ParseUint(idStr, 10, 64)
	if err != nil {
		c.JSON(http.StatusBadRequest, models.ErrorResponse{Error: "invalid item ID"})
		return
	}

	var req models.UpdateItemRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, models.ErrorResponse{Error: err.Error()})
		return
	}

	item, err := services.UpdateItem(id, &req)
	if err != nil {
		c.JSON(http.StatusNotFound, models.ErrorResponse{Error: "item not found"})
		return
	}
	c.JSON(http.StatusOK, item)
}

// DeleteItemHandler deletes an item by ID.
func DeleteItemHandler(c *gin.Context) {
	idStr := c.Param("id")
	id, err := strconv.ParseUint(idStr, 10, 64)
	if err != nil {
		c.JSON(http.StatusBadRequest, models.ErrorResponse{Error: "invalid item ID"})
		return
	}

	if err := services.DeleteItem(id); err != nil {
		c.JSON(http.StatusNotFound, models.ErrorResponse{Error: "item not found"})
		return
	}
	c.JSON(http.StatusNoContent, nil)
}