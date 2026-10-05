//! The bearer tokens Kippu issues and accepts.
//!
//! | Token | Signed by | Lifetime | Grants |
//! |---|---|---|---|
//! | root token | a root private key (held by a person) | ≤ `root.max_token_ttl_seconds` | everything |
//! | access token | the instance's token key | `auth.access_token_ttl_seconds` | the account's role |
//! | queue ticket | the instance's token key | `auth.queue_ticket_ttl_seconds` | a waiting-room position |
//! | admission pass | the instance's token key | `auth.admission_pass_ttl_seconds` | submitting purchases to one sale |
//!
//! All are EdDSA JWTs verified without any database lookup, so every instance can verify every
//! token. A `token_use` claim keeps one kind from being replayed as another.

use ed25519_dalek::{SigningKey, VerifyingKey};
use kippu_domain::account::{Account, Role};
use kippu_domain::{AccountId, Duration, OrganizationId, SaleId, Timestamp};
use serde::{Deserialize, Serialize};

use super::Principal;
use super::jwt::{self, JwtError};
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::keys::{ConfigError, parse_signing_key, parse_verifying_key};

/// Key id of tokens Kippu signs itself.
const INSTANCE_KID: &str = "kippu";
/// Key ids of root tokens are `root:<key name>`.
const ROOT_KID_PREFIX: &str = "root:";
/// Tolerated clock difference between a root token's minter and this server.
const CLOCK_SKEW_SECONDS: i64 = 60;

/// A freshly issued token.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct IssuedToken {
    /// The bearer token.
    pub token: String,
    /// When it stops being accepted.
    pub expires_at: Timestamp,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "token_use", rename_all = "snake_case")]
enum InstanceClaims {
    Access {
        iss: String,
        sub: AccountId,
        role: Role,
        #[serde(default)]
        orgs: Vec<OrganizationId>,
        iat: i64,
        exp: i64,
    },
    QueueTicket {
        iss: String,
        sub: AccountId,
        sale: SaleId,
        position: u64,
        iat: i64,
        exp: i64,
    },
    AdmissionPass {
        iss: String,
        sub: AccountId,
        sale: SaleId,
        iat: i64,
        exp: i64,
    },
}

impl InstanceClaims {
    const fn expires(&self) -> i64 {
        match self {
            Self::Access { exp, .. }
            | Self::QueueTicket { exp, .. }
            | Self::AdmissionPass { exp, .. } => *exp,
        }
    }

    fn issuer(&self) -> &str {
        match self {
            Self::Access { iss, .. }
            | Self::QueueTicket { iss, .. }
            | Self::AdmissionPass { iss, .. } => iss,
        }
    }
}

/// Claims of a root token. Any JWT library can produce them.
#[derive(Debug, Serialize, Deserialize)]
pub struct RootClaims {
    /// Always `"root"`.
    pub sub: String,
    /// The instance's issuer id, so a token for one deployment is useless at another.
    pub aud: String,
    /// Issued at, Unix seconds.
    pub iat: i64,
    /// Expires at, Unix seconds.
    pub exp: i64,
}

/// Issues and verifies tokens.
pub struct Tokens {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    issuer: String,
    root_keys: Vec<(String, VerifyingKey)>,
    root_max_ttl: i64,
    access_ttl: Duration,
    queue_ticket_ttl: Duration,
    admission_pass_ttl: Duration,
}

impl Tokens {
    /// Loads keys and lifetimes from configuration.
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let signing_key =
            parse_signing_key("keys.token_signing_key", &config.keys.token_signing_key)?;
        let root_keys = config
            .root
            .keys
            .iter()
            .map(|key| {
                let name = format!("root.keys[{}].public_key", key.name);
                Ok((
                    key.name.clone(),
                    parse_verifying_key(&name, &key.public_key)?,
                ))
            })
            .collect::<Result<_, ConfigError>>()?;
        Ok(Self {
            verifying_key: signing_key.verifying_key(),
            signing_key,
            issuer: config.issuer.id.clone(),
            root_keys,
            root_max_ttl: i64::from(config.root.max_token_ttl_seconds),
            access_ttl: Duration::seconds(i64::from(config.auth.access_token_ttl_seconds)),
            queue_ticket_ttl: Duration::seconds(i64::from(config.auth.queue_ticket_ttl_seconds)),
            admission_pass_ttl: Duration::seconds(i64::from(
                config.auth.admission_pass_ttl_seconds,
            )),
        })
    }

    fn issue(&self, claims: &InstanceClaims, expires_at: Timestamp) -> IssuedToken {
        IssuedToken {
            token: jwt::sign(&self.signing_key, INSTANCE_KID, claims),
            expires_at,
        }
    }

    /// Issues an access token carrying the account's role and organizations.
    pub fn issue_access(
        &self,
        account: &Account,
        organizations: Vec<OrganizationId>,
        now: Timestamp,
    ) -> IssuedToken {
        let expires_at = now + self.access_ttl;
        let claims = InstanceClaims::Access {
            iss: self.issuer.clone(),
            sub: account.id,
            role: account.role,
            orgs: organizations,
            iat: now.unix_seconds(),
            exp: expires_at.unix_seconds(),
        };
        self.issue(&claims, expires_at)
    }

    /// Issues a waiting-room queue ticket.
    pub fn issue_queue_ticket(
        &self,
        account: AccountId,
        sale: SaleId,
        position: u64,
        now: Timestamp,
    ) -> IssuedToken {
        let expires_at = now + self.queue_ticket_ttl;
        let claims = InstanceClaims::QueueTicket {
            iss: self.issuer.clone(),
            sub: account,
            sale,
            position,
            iat: now.unix_seconds(),
            exp: expires_at.unix_seconds(),
        };
        self.issue(&claims, expires_at)
    }

    /// Issues an admission pass to a sale.
    pub fn issue_admission_pass(
        &self,
        account: AccountId,
        sale: SaleId,
        now: Timestamp,
    ) -> IssuedToken {
        let expires_at = now + self.admission_pass_ttl;
        let claims = InstanceClaims::AdmissionPass {
            iss: self.issuer.clone(),
            sub: account,
            sale,
            iat: now.unix_seconds(),
            exp: expires_at.unix_seconds(),
        };
        self.issue(&claims, expires_at)
    }

    /// Turns a bearer token into the principal it proves.
    pub fn authenticate(&self, token: &str, now: Timestamp) -> ApiResult<Principal> {
        let unverified = jwt::parse(token).map_err(rejected)?;
        let kid = unverified.header.kid.as_deref().unwrap_or_default();
        if let Some(name) = kid.strip_prefix(ROOT_KID_PREFIX) {
            return self.authenticate_root(&unverified, name, now);
        }
        match self.verify_instance(&unverified, now)? {
            InstanceClaims::Access {
                sub, role, orgs, ..
            } => Ok(Principal::Account {
                id: sub,
                role,
                organizations: orgs,
            }),
            _ => Err(ApiError::unauthenticated("not an access token")),
        }
    }

    fn authenticate_root(
        &self,
        unverified: &jwt::Unverified<'_>,
        name: &str,
        now: Timestamp,
    ) -> ApiResult<Principal> {
        let (_, key) = self
            .root_keys
            .iter()
            .find(|(key_name, _)| key_name == name)
            .ok_or_else(|| ApiError::unauthenticated("unknown root key"))?;
        let claims: RootClaims = unverified.verify(key).map_err(rejected)?;
        let now = now.unix_seconds();
        let valid = claims.sub == "root"
            && claims.aud == self.issuer
            && claims.iat <= now.saturating_add(CLOCK_SKEW_SECONDS)
            && now < claims.exp
            && claims.exp.saturating_sub(claims.iat) <= self.root_max_ttl;
        if !valid {
            return Err(ApiError::unauthenticated(
                "root token is expired, too long-lived or meant for another deployment",
            ));
        }
        Ok(Principal::Root {
            key_name: name.to_owned(),
        })
    }

    fn verify_instance(
        &self,
        unverified: &jwt::Unverified<'_>,
        now: Timestamp,
    ) -> ApiResult<InstanceClaims> {
        if unverified.header.kid.as_deref() != Some(INSTANCE_KID) {
            return Err(ApiError::unauthenticated("unknown signing key"));
        }
        let claims: InstanceClaims = unverified.verify(&self.verifying_key).map_err(rejected)?;
        if claims.issuer() != self.issuer || claims.expires() <= now.unix_seconds() {
            return Err(ApiError::unauthenticated("token expired"));
        }
        Ok(claims)
    }

    /// Checks a queue ticket for `account` and `sale`, returning its position.
    pub fn verify_queue_ticket(
        &self,
        token: &str,
        account: AccountId,
        sale: SaleId,
        now: Timestamp,
    ) -> ApiResult<u64> {
        let unverified = jwt::parse(token).map_err(rejected)?;
        match self.verify_instance(&unverified, now)? {
            InstanceClaims::QueueTicket {
                sub,
                sale: for_sale,
                position,
                ..
            } if sub == account && for_sale == sale => Ok(position),
            _ => Err(ApiError::forbidden()),
        }
    }

    /// Checks an admission pass for `account` and `sale`.
    pub fn verify_admission_pass(
        &self,
        token: &str,
        account: AccountId,
        sale: SaleId,
        now: Timestamp,
    ) -> ApiResult<()> {
        let unverified = jwt::parse(token).map_err(rejected)?;
        match self.verify_instance(&unverified, now)? {
            InstanceClaims::AdmissionPass {
                sub,
                sale: for_sale,
                ..
            } if sub == account && for_sale == sale => Ok(()),
            _ => Err(ApiError::forbidden()),
        }
    }
}

fn rejected(error: JwtError) -> ApiError {
    ApiError::unauthenticated(error.to_string())
}

/// Mints a root token: what `kippu root-token` does with the root private key.
pub fn mint_root_token(
    key: &SigningKey,
    key_name: &str,
    audience: &str,
    now: Timestamp,
    ttl: Duration,
) -> String {
    let claims = RootClaims {
        sub: "root".to_owned(),
        aud: audience.to_owned(),
        iat: now.unix_seconds(),
        exp: (now + ttl).unix_seconds(),
    };
    jwt::sign(key, &format!("{ROOT_KID_PREFIX}{key_name}"), &claims)
}
