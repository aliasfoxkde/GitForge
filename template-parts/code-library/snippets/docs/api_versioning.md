# API Versioning Strategies

This document describes common API versioning strategies, their trade-offs, and implementation patterns.

## Overview

API versioning allows you to evolve your API while maintaining backward compatibility with existing clients. Choosing the right strategy depends on your team's workflow, client update patterns, and infrastructure constraints.

---

## Strategy 1: URL Path Versioning

**Pattern:** `/api/v1/resources`, `/api/v2/resources`

### Implementation

```
GET /api/v1/users/123
GET /api/v2/users/123
```

### Pros
- Most explicit - version is always visible in requests
- Easy to route and test
- Simple to understand for API consumers
- Works with caching

### Cons
- URL pollution
- Violates REST principle that URLs identify resources, not versions
- Client migration requires URL changes

### Implementation Example (Go)

```go
func (s *Server) RegisterRoutes() {
    v1 := s.router.Group("/api/v1")
    v1.GET("/users/:id", s.handleGetUserV1)

    v2 := s.router.Group("/api/v2")
    v2.GET("/users/:id", s.handleGetUserV2)
}
```

### Implementation Example (Python)

```python
from fastapi import FastAPI

app = FastAPI()

@app.get("/api/v1/users/{user_id}")
async def get_user_v1(user_id: int):
    return {"id": user_id, "name": "legacy"}

@app.get("/api/v2/users/{user_id}")
async def get_user_v2(user_id: int):
    return {"id": user_id, "name": "current", "email": "user@example.com"}
```

---

## Strategy 2: Header Versioning

**Pattern:** `API-Version: 2023-01-01` or `Accept: application/vnd.api.v2+json`

### Implementation

```http
GET /api/users/123
API-Version: 2023-01-01
```

### Pros
- Clean URLs that identify resources
- Date-based versions allow precision
- Supports content negotiation

### Cons
- Less visible, easy to miss
- Requires client to set headers correctly
- Caching complexity

### Implementation Example (Go)

```go
func (s *Server) handleGetUser(w http.ResponseWriter, r *http.Request) {
    version := r.Header.Get("API-Version")
    switch version {
    case "2023-01-01":
        s.handleGetUserV1(w, r)
    default:
        s.handleGetUserLatest(w, r)
    }
}
```

### Implementation Example (Python)

```python
from fastapi import Header, HTTPException

@app.get("/api/users/{user_id}")
async def get_user(
    user_id: int,
    api_version: str = Header(default="2024-01-01")
):
    if api_version == "2023-01-01":
        return get_user_v1_schema(user_id)
    return get_user_v2_schema(user_id)
```

---

## Strategy 3: Query Parameter Versioning

**Pattern:** `/api/users/123?version=2`

### Implementation

```http
GET /api/users/123?version=2
```

### Pros
- Simple to implement
- Easy to test in browsers
- Optional - defaults to latest version

### Cons
- Clutters URLs
- Not cache-friendly
- Easy to forget to include

### Implementation Example (Go)

```go
func (s *Server) handleGetUser(w http.ResponseWriter, r *http.Request) {
    version := r.URL.Query().Get("version")
    if version == "1" {
        s.handleGetUserV1(w, r)
        return
    }
    s.handleGetUserV2(w, r)
}
```

---

## Choosing a Strategy

| Strategy | Best For | Avoid When |
|----------|----------|------------|
| URL Path | Public APIs, clear version visibility needed | Frequent major versions |
| Header | Clean URLs, internal APIs | Browser testing, caching needs |
| Query Param | Simple APIs, optional versioning | High-traffic public APIs |

---

## Best Practices

### 1. Version Detection and Routing

```go
// Centralized version detection
func detectVersion(r *http.Request) string {
    // Check header first
    if v := r.Header.Get("API-Version"); v != "" {
        return v
    }
    // Check URL path
    if matches := pathVersionRegex.FindStringSubmatch(r.URL.Path); len(matches) > 1 {
        return matches[1]
    }
    // Default to latest
    return "latest"
}
```

### 2. Graceful Version Deprecation

```go
// Deprecation headers
w.Header().Set("Deprecation", "true")
w.Header().Set("Sunset", "Fri, 31 Dec 2024 23:59:59 GMT")
w.Header().Set("Link", "</api/v2/users>; rel=\"successor-version\"")
```

### 3. Feature Flags vs Versioning

For minor changes, consider feature flags instead of versioning:

```go
// Feature flag approach
if client.HasFeature("extended-user-schema") {
    return userV2Response
}
return userV1Response
```

### 4. Documentation Per Version

Maintain separate OpenAPI specs per version:

```
openapi/
  v1.yaml  # /api/v1 endpoints
  v2.yaml  # /api/v2 endpoints
  latest.yaml  # redirect to current
```

---

## Migration Checklist

When releasing a new API version:

- [ ] Document all changes from previous version
- [ ] Set deprecation timeline with sunset date
- [ ] Add `Deprecation` and `Sunset` response headers
- [ ] Notify clients via email/API status page
- [ ] Provide migration guide
- [ ] Update SDKs and client examples
- [ ] Monitor for deprecated version usage
