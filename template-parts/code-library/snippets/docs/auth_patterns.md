# Authentication Patterns

This document covers common authentication patterns: API keys, JWT, OAuth2, and mTLS.

## Overview

| Pattern | Use Case | Security Level |
|---------|----------|----------------|
| API Keys | Service-to-service, simple needs | Basic |
| JWT | Stateless auth, web/mobile apps | High |
| OAuth2 | Delegated access, third-party | High |
| mTLS | Service mesh, zero-trust networks | Very High |

---

## 1. API Key Authentication

Simple but effective for server-to-server communication.

### Implementation

```go
// Middleware to validate API key
func APIKeyAuth(next http.Handler) http.Handler {
    return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
        key := r.Header.Get("X-API-Key")
        if key == "" {
            key = r.URL.Query().Get("api_key")
        }

        if !isValidKey(key) {
            http.Error(w, "Unauthorized", http.StatusUnauthorized)
            return
        }

        next.ServeHTTP(w, r)
    })
}

func isValidKey(key string) bool {
    // In production: lookup in database, check permissions, rate limits
    return len(key) >= 32
}
```

```python
# Python middleware
from functools import wraps
from flask import request, jsonify

def require_api_key(f):
    @wraps(f)
    def decorated(*args, **kwargs):
        api_key = request.headers.get("X-API-Key") or request.args.get("api_key")
        if not api_key or not is_valid_key(api_key):
            return jsonify({"error": "Unauthorized"}), 401
        return f(*args, **kwargs)
    return decorated

def is_valid_key(key: str) -> bool:
    # In production: check database, permissions
    return len(key) >= 32
```

### Best Practices

- Use long, random keys (32+ characters)
- Store hashed, never plaintext
- Rotate keys periodically
- Use separate keys per service
- Never log API keys

---

## 2. JWT (JSON Web Tokens)

Stateless authentication with claims-based identity.

### Token Structure

```
Header.Payload.Signature
```

```json
{
  "alg": "HS256",
  "typ": "JWT"
}
-------------
{
  "sub": "user123",
  "iat": 1699900000,
  "exp": 1699903600,
  "roles": ["admin", "editor"]
}
```

### Implementation (Go)

```go
import (
    "github.com/golang-jwt/jwt/v5"
    "time"
)

type Claims struct {
    UserID string   `json:"sub"`
    Roles  []string `json:"roles"`
    jwt.RegisteredClaims
}

func GenerateToken(userID string, roles []string, secret string) (string, error) {
    claims := &Claims{
        UserID: userID,
        Roles:  roles,
        RegisteredClaims: jwt.RegisteredClaims{
            ExpiresAt: jwt.NewNumericDate(time.Now().Add(1 * time.Hour)),
            IssuedAt:  jwt.NewNumericDate(time.Now()),
        },
    }

    token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
    return token.SignedString([]byte(secret))
}

func ValidateToken(tokenString string, secret string) (*Claims, error) {
    token, err := jwt.ParseWithClaims(tokenString, &Claims{}, func(token *jwt.Token) (interface{}, error) {
        return []byte(secret), nil
    })

    if err != nil {
        return nil, err
    }

    if claims, ok := token.Claims.(*Claims); ok && token.Valid {
        return claims, nil
    }

    return nil, jwt.ErrSignatureInvalid
}
```

### Middleware (Go)

```go
func JWTAuth(secret string) func(http.Handler) http.Handler {
    return func(next http.Handler) http.Handler {
        return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
            authHeader := r.Header.Get("Authorization")
            if !strings.HasPrefix(authHeader, "Bearer ") {
                http.Error(w, "Missing token", http.StatusUnauthorized)
                return
            }

            claims, err := ValidateToken(authHeader[7:], secret)
            if err != nil {
                http.Error(w, "Invalid token", http.StatusUnauthorized)
                return
            }

            // Add user context
            ctx := context.WithValue(r.Context(), "userID", claims.UserID)
            ctx = context.WithValue(ctx, "roles", claims.Roles)
            next.ServeHTTP(w, r.WithContext(ctx))
        })
    }
}
```

### Implementation (Python)

```python
import jwt
import time
from functools import wraps
from flask import request, jsonify

def generate_token(user_id: str, roles: list[str], secret: str) -> str:
    payload = {
        "sub": user_id,
        "roles": roles,
        "iat": int(time.time()),
        "exp": int(time.time()) + 3600,
    }
    return jwt.encode(payload, secret, algorithm="HS256")

def validate_token(token: str, secret: str) -> dict | None:
    try:
        payload = jwt.decode(token, secret, algorithms=["HS256"])
        return payload
    except jwt.ExpiredSignatureError:
        return None
    except jwt.InvalidTokenError:
        return None

def require_jwt(f):
    @wraps(f)
    def decorated(*args, **kwargs):
        auth_header = request.headers.get("Authorization", "")
        if not auth_header.startswith("Bearer "):
            return jsonify({"error": "Missing token"}), 401

        token = auth_header[7:]
        payload = validate_token(token, "your-secret")
        if not payload:
            return jsonify({"error": "Invalid token"}), 401

        request.user_id = payload["sub"]
        request.roles = payload.get("roles", [])
        return f(*args, **kwargs)
    return decorated
```

### Best Practices

- Short expiration times (15min-1hr for access tokens)
- Use refresh tokens for long-lived sessions
- Always validate signature and expiration
- Include `iss` (issuer) and `aud` (audience) claims
- Keep tokens out of URLs (use headers)

---

## 3. OAuth2

Delegated authorization with scoped access tokens.

### Grant Types

| Grant | Use Case |
|-------|----------|
| Authorization Code | Web apps with server-side callback |
| PKCE | Mobile/SPA apps |
| Client Credentials | Service-to-service |
| Refresh Token | Obtaining new access tokens |

### Authorization Code Flow

```
Client                    Auth Server              Resource Server
  |                           |                          |
  |--- Authorization Request ->|                          |
  |<-- Authorization Code ---|                          |
  |--- Code + Client Secret ->|                          |
  |<-- Access Token ---------|                          |
  |                                                   |
  |--- Access Token -------------------------------->|
  |<-- Protected Resource ---------------------------|
```

### Implementation (Go - Client Credentials)

```go
type TokenResponse struct {
    AccessToken string `json:"access_token"`
    TokenType   string `json:"token_type"`
    ExpiresIn   int    `json:"expires_in"`
    Scope       string `json:"scope"`
}

func getServiceToken(clientID, clientSecret, tokenURL string) (*TokenResponse, error) {
    data := url.Values{}
    data.Set("grant_type", "client_credentials")
    data.Set("client_id", clientID)
    data.Set("client_secret", clientSecret)

    resp, err := http.PostForm(tokenURL, data)
    if err != nil {
        return nil, err
    }
    defer resp.Body.Close()

    var token TokenResponse
    if err := json.NewDecoder(resp.Body).Decode(&token); err != nil {
        return nil, err
    }

    return &token, nil
}
```

### Implementation (Python - Requests OAuth2)

```python
import requests
from requests.auth import OAuth2

# Client credentials flow
def get_service_token(client_id: str, client_secret: str, token_url: str) -> dict:
    response = requests.post(
        token_url,
        data={
            "grant_type": "client_credentials",
            "client_id": client_id,
            "client_secret": client_secret,
        },
    )
    response.raise_for_status()
    return response.json()

# Using the requests-oauthlib library
from requests_oauthlib import OAuth2Session

oauth = OAuth2Session(client_id, redirect_uri="http://callback.example.com")
authorization_url, state = oauth.authorization_url("https://auth.example.com/authorize")

# Fetch token
oauth.fetch_token(
    "https://auth.example.com/token",
    client_secret=client_secret,
    authorization_response=callback_url,
)
```

---

## 4. mTLS (Mutual TLS)

Both client and server present certificates, providing strong authentication in service meshes.

### Certificate Setup

```go
// Server configuration with mTLS
func newServerTLS(certFile, keyFile, caFile string) (*tls.Config, error) {
    serverCert, err := tls.LoadX509KeyPair(certFile, keyFile)
    if err != nil {
        return nil, err
    }

    caCert, err := os.ReadFile(caFile)
    if err != nil {
        return nil, err
    }

    caCertPool := x509.NewCertPool()
    caCertPool.AppendCertsFromPEM(caCert)

    return &tls.Config{
        Certificates: []tls.Certificate{serverCert},
        ClientCAs:    caCertPool,
        ClientAuth:   tls.RequireAndVerifyClientCert,
    }, nil
}

// Start server with mTLS
func serveMTLS(addr, certFile, keyFile, caFile string, handler http.Handler) error {
    tlsConfig, err := newServerTLS(certFile, keyFile, caFile)
    if err != nil {
        return err
    }

    server := &http.Server{
        Addr:      addr,
        TLSConfig: tlsConfig,
        Handler:   handler,
    }

    return server.ListenAndServeTLS(certFile, keyFile)
}
```

### Python (using ssl module)

```python
import ssl

def create_mtls_context(
    server_cert: str,
    server_key: str,
    client_cert: str | None = None,
    client_key: str | None = None,
    ca_file: str | None = None,
) -> ssl.SSLContext:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(server_cert, server_key)

    if ca_file:
        context.load_verify_locations(ca_file)
        context.verify_mode = ssl.CERT_REQUIRED

    if client_cert and client_key:
        context.load_cert_chain(client_cert, client_key)

    return context

# Usage with aiohttp
import aiohttp

async def make_mtls_request(url: str, client_cert: str, client_key: str, ca: str):
    ssl_context = create_mtls_context(client_cert, client_key, ca_file=ca)
    connector = aiohttp.TCPConnector(ssl=ssl_context)
    async with aiohttp.ClientSession(connector=connector) as session:
        response = await session.get(url)
        return await response.json()
```

---

## Security Checklist

- [ ] All auth tokens/keys stored securely, never in code
- [ ] TLS required for all authentication flows
- [ ] Failed auth attempts logged and rate-limited
- [ ] Tokens have appropriate expiration
- [ ] Sensitive data not logged
- [ ] CORS configured for browser clients
- [ ] Redirect URIs validated (OAuth2)
- [ ] Client secrets rotated periodically
- [ ] Certificate rotation process in place (mTLS)
