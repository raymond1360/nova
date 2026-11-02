//! Purpose-separated credentials. Native entropy is compiled, not used by mock tests.
use super::Result;
use ring::rand::{SecureRandom, SystemRandom};
pub enum Model {}
pub enum Control {}
pub struct RunNonce<P> {
    value: [u8; 64],
    _purpose: std::marker::PhantomData<P>,
}
impl<P> RunNonce<P> {
    pub fn generate() -> Result<Self> {
        let mut raw = [0; 32];
        if SystemRandom::new().fill(&mut raw).is_err() {
            raw.fill(0);
            return Err("entropy unavailable".into());
        }
        Self::encode(raw)
    }
    fn encode(mut raw: [u8; 32]) -> Result<Self> {
        let mut value = [0; 64];
        for (i, b) in raw.iter().enumerate() {
            value[2 * i] = b"0123456789abcdef"[(b >> 4) as usize];
            value[2 * i + 1] = b"0123456789abcdef"[(b & 15) as usize];
        }
        raw.fill(0);
        Ok(Self {
            value,
            _purpose: std::marker::PhantomData,
        })
    }
    // Only fixed adapters in this module tree can obtain temporary credential bytes.
    pub(super) fn with_bytes<T>(&self, f: impl FnOnce(&[u8]) -> T) -> T {
        f(&self.value)
    }
    pub(super) fn validate_external(value: &[u8]) -> Result<()> {
        if value.len() != 64
            || !value
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
            || value == super::opencode_broker::INJECTED_NONCE
            || value.starts_with(b"UNPREPARED")
        {
            return Err("reserved or invalid credential".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn injected(fill: impl FnOnce(&mut [u8; 32]) -> Result<()>) -> Result<Self> {
        let mut raw = [0; 32];
        fill(&mut raw)?;
        Self::encode(raw)
    }
}
impl<P> Drop for RunNonce<P> {
    fn drop(&mut self) {
        self.value.fill(0);
    }
}
pub struct RunSecrets {
    pub model: RunNonce<Model>,
    pub control: RunNonce<Control>,
}
impl RunSecrets {
    pub fn generate() -> Result<Self> {
        let s = Self {
            model: RunNonce::generate()?,
            control: RunNonce::generate()?,
        };
        s.distinct()?;
        Ok(s)
    }
    pub(super) fn distinct(&self) -> Result<()> {
        if self
            .model
            .with_bytes(|m| self.control.with_bytes(|c| m == c))
        {
            return Err("credentials must differ".into());
        }
        Ok(())
    }
}
// No Debug/Serialize or environment/CLI import. Best-effort memory clearing is not a
// promise that copies in HTTP libraries or OS buffers have been erased.
pub(super) fn basic(secret: &RunNonce<Control>) -> Result<reqwest::header::HeaderValue> {
    secret.with_bytes(|s| {
        RunNonce::<Control>::validate_external(s)?;
        let mut bytes = b"opencode:".to_vec();
        bytes.extend_from_slice(s);
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::from("Basic ");
        for c in bytes.chunks(3) {
            let n = ((c[0] as u32) << 16)
                | ((c.get(1).copied().unwrap_or(0) as u32) << 8)
                | c.get(2).copied().unwrap_or(0) as u32;
            encoded.push(alphabet[((n >> 18) & 63) as usize] as char);
            encoded.push(alphabet[((n >> 12) & 63) as usize] as char);
            encoded.push(if c.len() > 1 {
                alphabet[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            encoded.push(if c.len() > 2 {
                alphabet[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        bytes.fill(0);
        let mut h =
            reqwest::header::HeaderValue::from_str(&encoded).map_err(|_| "credential encoding")?;
        h.set_sensitive(true);
        Ok(h)
    })
}
