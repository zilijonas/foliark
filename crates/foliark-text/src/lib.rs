//! Font registry (spec §1.4) and the authoritative paragraph layout (spec §6.1, §6.3, §6.4).
mod layout;
mod registry;

pub use layout::*;
pub use registry::*;

/// FNV-1a 64: a stable, dependency-free hash for ids and plan fingerprints.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
        self
    }

    /// Length-prefixed, so adjacent fields cannot run together.
    pub fn field(&mut self, b: &[u8]) -> &mut Self {
        self.bytes(&(b.len() as u64).to_le_bytes()).bytes(b)
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Self::new()
    }
}
