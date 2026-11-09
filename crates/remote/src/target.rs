//! A paired destination exports its capability only through explicit link methods.
use crate::{Error, Invitation};

#[derive(Clone, Debug)]
pub struct PairedTarget {
    origin: String,
    invitation: Invitation,
}
impl PairedTarget {
    /// Import a bounded private export; capabilities never appear in errors.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_link_file(path: &std::path::Path) -> Result<Self, Error> {
        use std::io::Read;
        let file = std::fs::File::open(path).map_err(|_| Error::Invitation)?;
        if !file.metadata().map_err(|_| Error::Invitation)?.is_file() {
            return Err(Error::Invitation);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Invitation)?;
        }
        let mut bytes = zeroize::Zeroizing::new(String::new());
        file.take(2049)
            .read_to_string(&mut bytes)
            .map_err(|_| Error::Invitation)?;
        if bytes.len() > 2048 {
            return Err(Error::Invitation);
        }
        Self::parse_link(bytes.trim())
    }

    pub fn new(origin: &str, invitation: Invitation) -> Result<Self, Error> {
        Ok(Self {
            origin: normalized_origin(origin)?,
            invitation,
        })
    }
    /// One address of an already-paired host. Plain HTTP is allowed on private
    /// addresses because only Noise records ever cross this socket.
    pub fn for_route(origin: &str, invitation: Invitation) -> Result<Self, Error> {
        Ok(Self {
            origin: crate::normalized_route_origin(origin)?,
            invitation,
        })
    }
    pub fn parse_link(link: &str) -> Result<Self, Error> {
        if link.len() > 2048 {
            return Err(Error::Endpoint);
        }
        let mut url = url::Url::parse(link).map_err(|_| Error::Endpoint)?;
        let fragment = url.fragment().ok_or(Error::Invitation)?;
        let invitation = Invitation::parse_fragment(&format!("#{fragment}"))?;
        url.set_fragment(None);
        // A native client can pair over LAN or a tailnet. A browser still has
        // to load its application from a trusted HTTPS origin.
        #[cfg(not(target_arch = "wasm32"))]
        return Self::for_route(url.as_str(), invitation);
        #[cfg(target_arch = "wasm32")]
        Self::new(url.as_str(), invitation)
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn invitation(&self) -> &Invitation {
        &self.invitation
    }
    pub fn socket_url(&self) -> String {
        let scheme = if self.origin.starts_with("https:") {
            "wss"
        } else {
            "ws"
        };
        format!(
            "{scheme}{}{}{}",
            &self.origin[self.origin.find(':').unwrap()..],
            "/api/paired-rpc/",
            self.invitation.id.as_str()
        )
    }
    /// Private export. Do not log or put the resulting link in ordinary settings.
    pub fn export_link(&self) -> Result<String, Error> {
        Ok(format!("{}/{}", self.origin, self.invitation.fragment()?))
    }
}

/// Validate a public application origin without creating or exporting a capability.
pub fn normalized_origin(origin: &str) -> Result<String, Error> {
    let mut url = url::Url::parse(origin).map_err(|_| Error::Endpoint)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Endpoint);
    }
    let scheme = match url.scheme() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        _ => return Err(Error::Endpoint),
    };
    // Remote app bundles must arrive over trusted TLS. Local development can
    // use HTTP while Noise still authenticates and encrypts the RPC session.
    if scheme == "http" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(Error::Endpoint);
    }
    url.set_scheme(scheme).map_err(|_| Error::Endpoint)?;
    Ok(url.origin().ascii_serialization())
}
