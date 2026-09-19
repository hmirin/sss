use anyhow::{bail, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum Rule {
    #[default]
    Inherit,
    None,
    Basic {
        username: String,
        password: String,
    },
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    #[serde(default)]
    pub view: Rule,
    #[serde(default)]
    pub write: Rule,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum StoredRule {
    #[default]
    Inherit,
    None,
    Basic {
        username: String,
        hash: String,
    },
}
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct StoredAccess {
    pub view: StoredRule,
    pub write: StoredRule,
}
pub fn split(value: &str) -> Result<(&str, &str)> {
    let Some((u, p)) = value.split_once(':') else {
        bail!("Basic authentication must be username:password")
    };
    if u.is_empty()
        || p.is_empty()
        || u.len() > 256
        || p.len() > 4096
        || u.chars().chain(p.chars()).any(char::is_control)
    {
        bail!("invalid Basic authentication credentials")
    }
    Ok((u, p))
}
impl Rule {
    pub fn argument(v: &str) -> Result<Self> {
        Ok(match v {
            "none" => Self::None,
            "inherit" => Self::Inherit,
            _ => {
                let (u, p) = split(v)?;
                Self::Basic {
                    username: u.into(),
                    password: p.into(),
                }
            }
        })
    }
    pub fn store(&self) -> Result<StoredRule> {
        Ok(match self {
            Self::Inherit => StoredRule::Inherit,
            Self::None => StoredRule::None,
            Self::Basic { username, password } => {
                split(&format!("{username}:{password}"))?;
                if username.contains(':') {
                    bail!("username must not contain a colon")
                }
                let salt = SaltString::generate(&mut rand::rngs::OsRng);
                let hash = Argon2::default()
                    .hash_password(password.as_bytes(), &salt)
                    .map_err(|_| anyhow::anyhow!("password hashing failed"))?
                    .to_string();
                StoredRule::Basic {
                    username: username.clone(),
                    hash,
                }
            }
        })
    }
}
impl StoredRule {
    pub fn matches(&self, credential: Option<&str>) -> bool {
        let Self::Basic { username, hash } = self else {
            return false;
        };
        let Some((u, p)) = credential.and_then(|s| s.split_once(':')) else {
            return false;
        };
        u == username
            && PasswordHash::new(hash)
                .is_ok_and(|h| Argon2::default().verify_password(p.as_bytes(), &h).is_ok())
    }
}
impl Access {
    pub fn store(&self) -> Result<StoredAccess> {
        Ok(StoredAccess {
            view: self.view.store()?,
            write: self.write.store()?,
        })
    }
}
