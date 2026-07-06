package api

import (
	"github.com/example/api-service/src/config"
	"github.com/gin-gonic/gin"
)

func NewRouter(cfg *config.Config) *gin.Engine {
	if cfg.Env == "production" {
		gin.SetMode(gin.ReleaseMode)
	}

	router := gin.New()

	// Global middleware
	router.Use(gin.Recovery())
	router.Use(RequestLogger())

	// Health check (no auth required)
	router.GET("/health", HealthHandler)

	// API v1 routes (protected)
	v1 := router.Group("/api/v1")
	v1.Use(AuthMiddleware())
	v1.Use(RateLimitMiddleware())
	{
		v1.GET("/items", ListItemsHandler)
		v1.GET("/items/:id", GetItemHandler)
		v1.POST("/items", CreateItemHandler)
		v1.PUT("/items/:id", UpdateItemHandler)
		v1.DELETE("/items/:id", DeleteItemHandler)
	}

	return router
}