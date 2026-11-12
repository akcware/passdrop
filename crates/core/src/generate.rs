//! `x-generate`: values passdrop creates itself so nobody types or sees them.

use base64::Engine;
use secrecy::SecretString;
use zeroize::Zeroize;

pub const MIN_BYTES: usize = 16;
pub const MAX_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateSpec {
    Hex(usize),
    Base64(usize),
    Uuid,
}

impl GenerateSpec {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "uuid" {
            return Some(Self::Uuid);
        }
        let (kind, n) = s.split_once(':')?;
        let n: usize = n.parse().ok()?;
        if !(MIN_BYTES..=MAX_BYTES).contains(&n) {
            return None;
        }
        match kind {
            "hex" => Some(Self::Hex(n)),
            "base64" => Some(Self::Base64(n)),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Hex(n) => format!("{n} random bytes (hex)"),
            Self::Base64(n) => format!("{n} random bytes (base64)"),
            Self::Uuid => "random UUID (v4)".to_string(),
        }
    }

    pub fn generate(&self) -> Result<SecretString, getrandom::Error> {
        match *self {
            Self::Uuid => Ok(SecretString::from(uuid::Uuid::new_v4().to_string())),
            Self::Hex(n) => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let mut buf = vec![0u8; n];
                getrandom::fill(&mut buf)?;
                let mut s = String::with_capacity(n * 2);
                for b in &buf {
                    s.push(HEX[(b >> 4) as usize] as char);
                    s.push(HEX[(b & 0x0f) as usize] as char);
                }
                buf.zeroize();
                Ok(SecretString::from(s))
            }
            Self::Base64(n) => {
                let mut buf = vec![0u8; n];
                getrandom::fill(&mut buf)?;
                let s = base64::engine::general_purpose::STANDARD.encode(&buf);
                buf.zeroize();
                Ok(SecretString::from(s))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn parse_forms() {
        assert_eq!(GenerateSpec::parse("hex:32"), Some(GenerateSpec::Hex(32)));
        assert_eq!(GenerateSpec::parse("base64:16"), Some(GenerateSpec::Base64(16)));
        assert_eq!(GenerateSpec::parse("uuid"), Some(GenerateSpec::Uuid));
        assert_eq!(GenerateSpec::parse("hex:8"), None);
        assert_eq!(GenerateSpec::parse("hex:129"), None);
        assert_eq!(GenerateSpec::parse("oct:32"), None);
        assert_eq!(GenerateSpec::parse("hex"), None);
    }

    #[test]
    fn generated_shapes() {
        let h = GenerateSpec::Hex(32).generate().unwrap();
        assert_eq!(h.expose_secret().len(), 64);
        assert!(h.expose_secret().chars().all(|c| c.is_ascii_hexdigit()));
        let b = GenerateSpec::Base64(32).generate().unwrap();
        assert_eq!(b.expose_secret().len(), 44);
        let u = GenerateSpec::Uuid.generate().unwrap();
        assert!(uuid::Uuid::parse_str(u.expose_secret()).is_ok());
        let h2 = GenerateSpec::Hex(32).generate().unwrap();
        assert_ne!(h.expose_secret(), h2.expose_secret());
    }
}
