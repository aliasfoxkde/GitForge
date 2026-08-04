# API Documentation

## Overview

Describe your API here.

## Endpoints

### Health Check

**GET** `/health`

Returns the health status of the service.

**Response:**
```json
{
  "status": "healthy",
  "version": "0.1.0"
}
```

### Resources

**POST** `/resources`

Create a new resource.

**Request:**
```json
{
  "name": "string",
  "description": "string"
}
```

**Response:**
```json
{
  "id": "uuid",
  "name": "string",
  "description": "string",
  "created_at": "ISO8601"
}
```

**GET** `/resources/{id}`

Get a resource by ID.

**Response:**
```json
{
  "id": "uuid",
  "name": "string",
  "description": "string",
  "created_at": "ISO8601"
}
```

**DELETE** `/resources/{id}`

Delete a resource by ID.

**Response:** `204 No Content`
