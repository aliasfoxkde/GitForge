//! API authentication

use chrono::Utc;
use gitforce_common::{Error, UserId};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// JWT claims for API authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,
    /// User ID
    pub user_id: UserId,
    /// Username
    pub username: String,
    /// Role
    pub role: String,
    /// Expiration time (as UTC timestamp)
    pub exp: i64,
    /// Issued at
    pub iat: i64,
}

impl Claims {
    /// Create new claims
    pub fn new(user_id: UserId, username: &str, role: &str, expiry_hours: i64) -> Self {
        let now = Utc::now();
        Self {
            sub: user_id.to_string(),
            user_id,
            username: username.to_string(),
            role: role.to_string(),
            exp: now.timestamp() + (expiry_hours * 3600),
            iat: now.timestamp(),
        }
    }

    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() > self.exp
    }
}

/// API authentication handler
#[derive(Clone)]
pub struct ApiAuth {
    #[allow(dead_code)]
    jwt_secret: String,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl ApiAuth {
    /// Create a new API authenticator
    pub fn new(jwt_secret: &str) -> Self {
        let encoding_key = EncodingKey::from_secret(jwt_secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(jwt_secret.as_bytes());

        Self {
            jwt_secret: jwt_secret.to_string(),
            encoding_key,
            decoding_key,
        }
    }

    /// Generate a JWT token for a user
    pub fn generate_token(
        &self,
        user_id: UserId,
        username: &str,
        role: &str,
    ) -> Result<String, Error> {
        let claims = Claims::new(user_id, username, role, 24); // 24 hour expiry

        let token = encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| Error::auth(format!("failed to generate token: {}", e)))?;

        Ok(token)
    }

    /// Validate and decode a JWT token
    pub fn validate_token(&self, token: &str) -> Result<Claims, Error> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &Validation::default())
            .map_err(|e| Error::auth(format!("invalid token: {}", e)))?;

        let claims = token_data.claims;

        if claims.is_expired() {
            return Err(Error::auth("token expired".to_string()));
        }

        Ok(claims)
    }

    /// Extract token from Authorization header
    pub fn extract_token(auth_header: &str) -> Option<&str> {
        auth_header.strip_prefix("Bearer ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claims_creation() {
        let user_id = UserId::new();
        let claims = Claims::new(user_id, "user1", "developer", 2);
        assert_eq!(claims.username, "user1");
        assert_eq!(claims.role, "developer");
    }

    #[test]
    fn test_claims_expiry() {
        let claims = Claims::new(UserId::new(), "test", "admin", 1);
        assert!(!claims.is_expired());

        let expired_claims = Claims {
            exp: Utc::now().timestamp() - 3600,
            iat: Utc::now().timestamp() - 7200,
            ..claims
        };
        assert!(expired_claims.is_expired());
    }

    #[test]
    fn test_token_generation_and_validation() {
        let auth = ApiAuth::new("test-secret");
        let user_id = UserId::new();

        let token = auth.generate_token(user_id, "testuser", "admin").unwrap();
        assert!(!token.is_empty());

        let claims = auth.validate_token(&token).unwrap();
        assert_eq!(claims.username, "testuser");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_extract_token() {
        assert_eq!(ApiAuth::extract_token("Bearer abc123"), Some("abc123"));
        assert_eq!(ApiAuth::extract_token("abc123"), None);
        assert_eq!(ApiAuth::extract_token("Basic abc"), None);
    }

    #[test]
    fn test_invalid_token() {
        let auth = ApiAuth::new("test-secret");
        let result = auth.validate_token("invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret_validation() {
        let auth1 = ApiAuth::new("secret1");
        let auth2 = ApiAuth::new("secret2");
        let user_id = UserId::new();
        let token = auth1.generate_token(user_id, "testuser", "admin").unwrap();
        let result = auth2.validate_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_claims_with_different_roles() {
        let user_id = UserId::new();
        let admin_claims = Claims::new(user_id, "admin", "admin", 24);
        let user_claims = Claims::new(user_id, "user", "user", 24);
        assert_eq!(admin_claims.role, "admin");
        assert_eq!(user_claims.role, "user");
    }

    #[test]
    fn test_claims_debug() {
        let claims = Claims::new(UserId::new(), "test", "admin", 1);
        let debug_str = format!("{:?}", claims);
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_token_with_long_expiry() {
        let auth = ApiAuth::new("test-secret");
        let user_id = UserId::new();
        let token = auth.generate_token(user_id, "user", "admin").unwrap();
        let claims = auth.validate_token(&token).unwrap();
        // Token should be valid and not expired
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_claims_iat_and_exp() {
        let before = Utc::now().timestamp();
        let claims = Claims::new(UserId::new(), "test", "admin", 2);
        let after = Utc::now().timestamp();
        // iat should be current time
        assert!(claims.iat >= before);
        assert!(claims.iat <= after);
        // exp should be iat + 2 hours
        assert_eq!(claims.exp, claims.iat + (2 * 3600));
    }
}
