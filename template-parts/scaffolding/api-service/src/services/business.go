package services

import (
	"errors"
	"sync"
	"time"

	"github.com/example/api-service/src/models"
)

var (
	ErrItemNotFound = errors.New("item not found")
)

// In-memory store for scaffold simplicity.
// In production, replace with database calls using sqlx or GORM.
type store struct {
	mu    sync.RWMutex
	items map[uint64]*models.Item
	next  uint64
}

var mem = &store{
	items: make(map[uint64]*models.Item),
	next:  1,
}

// ListItems returns all items.
func ListItems() ([]models.Item, error) {
	mem.mu.RLock()
	defer mem.mu.RUnlock()

	items := make([]models.Item, 0, len(mem.items))
	for _, v := range mem.items {
		items = append(items, *v)
	}
	return items, nil
}

// GetItem returns a single item by ID.
func GetItem(id uint64) (*models.Item, error) {
	mem.mu.RLock()
	defer mem.mu.RUnlock()

	item, ok := mem.items[id]
	if !ok {
		return nil, ErrItemNotFound
	}
副本 := *item
	return &副本, nil
}

// CreateItem creates a new item.
func CreateItem(req *models.CreateItemRequest) (*models.Item, error) {
	mem.mu.Lock()
	defer mem.mu.Unlock()

	now := time.Now()
	item := &models.Item{
		ID:          mem.next,
		Name:        req.Name,
		Description: req.Description,
		CreatedAt:   now,
		UpdatedAt:   now,
	}
	mem.items[mem.next] = item
	mem.next++
	return item, nil
}

// UpdateItem updates an existing item.
func UpdateItem(id uint64, req *models.UpdateItemRequest) (*models.Item, error) {
	mem.mu.Lock()
	defer mem.mu.Unlock()

	item, ok := mem.items[id]
	if !ok {
		return nil, ErrItemNotFound
	}

	if req.Name != nil {
		item.Name = *req.Name
	}
	if req.Description != nil {
		item.Description = *req.Description
	}
	item.UpdatedAt = time.Now()

副本 := *item
	return &副本, nil
}

// DeleteItem deletes an item by ID.
func DeleteItem(id uint64) error {
	mem.mu.Lock()
	defer mem.mu.Unlock()

	if _, ok := mem.items[id]; !ok {
		return ErrItemNotFound
	}
	delete(mem.items, id)
	return nil
}