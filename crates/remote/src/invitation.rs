use crate::Error;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PairId(String);
impl PairId {
    pub fn generate() -> Result<Self, Error> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
        Ok(Self(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }
    pub fn parse(value: &str) -> Result<Self, Error> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(Error::Invitation);
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub struct PairSecret(Zeroizing<[u8; 32]>);
impl std::fmt::Debug for PairSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairSecret([redacted])")
    }
}
impl PairSecret {
    pub fn generate() -> Result<Self, Error> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(bytes.as_mut()).map_err(|_| Error::Random)?;
        Ok(Self(bytes))
    }
    pub fn parse(value: &str) -> Result<Self, Error> {
        if value.len() != 43 {
            return Err(Error::Invitation);
        }
        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| Error::Invitation)?,
        );
        let mut key = Zeroizing::new([0u8; 32]);
        if decoded.len() != key.len() {
            return Err(Error::Invitation);
        }
        key.copy_from_slice(&decoded);
        Ok(Self(key))
    }
    /// Explicit export for an invitation or private credential store. Never log.
    pub fn expose(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.as_ref())
    }
    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Transfer only through a private URL fragment or equivalent out-of-band path.
#[derive(Clone, Debug)]
pub struct Invitation {
    pub id: PairId,
    pub secret: PairSecret,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireInvitation {
    v: u32,
    id: String,
    secret: String,
}
impl Invitation {
    pub fn generate() -> Result<Self, Error> {
        Ok(Self {
            id: PairId::generate()?,
            secret: PairSecret::generate()?,
        })
    }
    pub fn fragment(&self) -> Result<String, Error> {
        let wire = WireInvitation {
            v: 1,
            id: self.id.as_str().into(),
            secret: self.secret.expose(),
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| Error::Invitation)?);
        Ok(format!(
            "#pair={}",
            URL_SAFE_NO_PAD.encode(bytes.as_slice())
        ))
    }
    pub fn parse_fragment(value: &str) -> Result<Self, Error> {
        let value = value.strip_prefix("#pair=").ok_or(Error::Invitation)?;
        if value.len() > 512 {
            return Err(Error::Invitation);
        }
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| Error::Invitation)?,
        );
        let wire: WireInvitation = serde_json::from_slice(&bytes).map_err(|_| Error::Invitation)?;
        if wire.v != 1 {
            return Err(Error::Invitation);
        }
        Ok(Self {
            id: PairId::parse(&wire.id)?,
            secret: PairSecret::parse(&wire.secret)?,
        })
    }
}
