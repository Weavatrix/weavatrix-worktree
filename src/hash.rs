use core::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use sha2::{Digest, Sha256};

/// Exact 32-byte SHA-256 digest represented as lowercase hexadecimal on wire.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sha256Hash([u8; 32]);

impl Sha256Hash {
    #[must_use]
    pub fn compute(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn parse(value: &str) -> Result<Self, ParseSha256Error> {
        value.parse()
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for Sha256Hash {
    type Err = ParseSha256Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 {
            return Err(ParseSha256Error);
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sha256Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Sha256Hash(\"{self}\")")
    }
}

impl Serialize for Sha256Hash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Sha256Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(Sha256Visitor)
    }
}

struct Sha256Visitor;

impl Visitor<'_> for Sha256Visitor {
    type Value = Sha256Hash;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("64 lowercase hexadecimal SHA-256 characters")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Sha256Hash::parse(value).map_err(E::custom)
    }
}

/// A malformed lowercase SHA-256 hexadecimal value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseSha256Error;

impl fmt::Display for ParseSha256Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SHA-256 must contain 64 lowercase hexadecimal characters")
    }
}

impl std::error::Error for ParseSha256Error {}

fn nibble(byte: u8) -> Result<u8, ParseSha256Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(ParseSha256Error),
    }
}

pub(crate) struct Sha256Hasher(Sha256);

impl Sha256Hasher {
    pub(crate) fn new() -> Self {
        Self(Sha256::new())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub(crate) fn finish(self) -> Sha256Hash {
        Sha256Hash::from_bytes(self.0.finalize().into())
    }
}

#[cfg(test)]
mod tests {
    use crate::hash::{Sha256Hash, Sha256Hasher};

    #[test]
    fn matches_standard_vectors_and_incremental_updates() {
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(Sha256Hash::compute(b"abc").to_string(), expected);

        let mut hasher = Sha256Hasher::new();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(hasher.finish().to_string(), expected);
    }

    #[test]
    fn parsing_is_exact_and_lowercase() {
        let digest = Sha256Hash::compute(b"");
        assert_eq!(digest.to_string().parse(), Ok(digest));
        assert!(
            digest
                .to_string()
                .to_uppercase()
                .parse::<Sha256Hash>()
                .is_err()
        );
        assert!("00".parse::<Sha256Hash>().is_err());
    }
}
