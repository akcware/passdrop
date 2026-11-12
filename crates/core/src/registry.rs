//! Known key formats: where to get a key and what it usually looks like.
//! Seeded from tokenstash (MIT); see THIRD_PARTY_NOTICES.md. Patterns only ever warn.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use crate::spec::{Field, SinkSpec, is_safe_https_url};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Entry {
    /// Env-var name, the lookup key.
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub generate: Option<String>,
}

#[derive(Deserialize)]
struct RegistryFile {
    entries: Vec<Entry>,
}

pub struct Registry {
    entries: Vec<Entry>,
    patterns: Vec<Option<Regex>>,
}

#[derive(Debug, Clone, Copy)]
pub struct RegistryHit<'a> {
    pub entry: &'a Entry,
    pub pattern: Option<&'a Regex>,
}

const BUILTIN: &str = include_str!("../registry/keys.json");

impl Registry {
    pub fn builtin() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(|| Registry::from_json(BUILTIN).expect("embedded registry is valid (checked by tests)"))
    }

    pub fn empty() -> Registry {
        Registry { entries: Vec::new(), patterns: Vec::new() }
    }

    pub fn from_json(s: &str) -> Result<Registry, String> {
        let file: RegistryFile = serde_json::from_str(s).map_err(|e| format!("registry JSON: {e}"))?;
        let mut seen = HashSet::new();
        let mut patterns = Vec::with_capacity(file.entries.len());
        for e in &file.entries {
            if !seen.insert(e.name.clone()) {
                return Err(format!("registry: duplicate entry {}", e.name));
            }
            if let Some(url) = &e.url
                && !is_safe_https_url(url) {
                    return Err(format!("registry: {} has a non-https url", e.name));
                }
            let re = match &e.pattern {
                Some(p) => Some(Regex::new(p).map_err(|err| format!("registry: {} pattern: {err}", e.name))?),
                None => None,
            };
            patterns.push(re);
        }
        Ok(Registry { entries: file.entries, patterns })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<RegistryHit<'_>> {
        let i = self.entries.iter().position(|e| e.name == name)?;
        Some(RegistryHit { entry: &self.entries[i], pattern: self.patterns[i].as_ref() })
    }

    /// `x-known` first, otherwise the env-var name a sink writes to.
    pub fn lookup_for_field(&self, field: &Field) -> Option<RegistryHit<'_>> {
        if let Some(k) = &field.known {
            return self.get(k);
        }
        match &field.sink {
            Some(SinkSpec::Dotenv { key, .. }) => self.get(key),
            Some(SinkSpec::GhSecret { name, .. }) | Some(SinkSpec::VercelEnv { name, .. }) => self.get(name),
            _ => None,
        }
    }

    /// Heuristic for echoed values: does this look like a secret key?
    /// Only entries for secret-like names count, so URLs, IDs and publishable keys don't trigger it.
    pub fn looks_like_key(&self, value: &str) -> bool {
        if value.len() < 20 || value.chars().any(char::is_whitespace) {
            return false;
        }
        self.entries
            .iter()
            .zip(&self.patterns)
            .filter(|(e, _)| is_secret_like(&e.name))
            .any(|(_, re)| re.as_ref().is_some_and(|re| re.is_match(value)))
    }
}

fn is_secret_like(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    let public = ["_URL", "_DSN", "_ID", "_SID"].iter().any(|s| n.ends_with(s));
    !(public || n.contains("PUBLISHABLE") || n.starts_with("NEXT_PUBLIC_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_loads_every_pattern_and_url() {
        let r = Registry::from_json(BUILTIN).expect("builtin registry");
        assert!(r.entries().len() >= 70);
        for e in r.entries() {
            if let Some(u) = &e.url {
                assert!(u.starts_with("https://"), "{}", e.name);
            }
            assert!(!e.steps.is_empty(), "{} has no steps", e.name);
        }
    }

    #[test]
    fn generic_entries_have_no_url() {
        let r = Registry::builtin();
        for name in ["DATABASE_URL", "SESSION_SECRET", "JWT_SECRET", "ENCRYPTION_KEY"] {
            assert!(r.get(name).unwrap().entry.url.is_none(), "{name}");
        }
    }

    #[test]
    fn lookup_and_heuristic() {
        let r = Registry::builtin();
        let hit = r.get("OPENAI_API_KEY").unwrap();
        assert!(hit.pattern.unwrap().is_match("sk-proj-abc"));
        assert!(r.looks_like_key("sk-ant-api03-aaaaaaaaaaaaaaaaaaaa"));
        assert!(!r.looks_like_key("57246542-96fe-1a63-e053-0824d011072a"));
        assert!(!r.looks_like_key("https://abcdefghij.supabase.co"));
        assert!(!r.looks_like_key("sk-short"));
    }

    #[test]
    fn rejects_bad_registry() {
        assert!(Registry::from_json(r#"{"entries":[{"name":"A","provider":"p","pattern":"("}]}"#).is_err());
        assert!(Registry::from_json(r#"{"entries":[{"name":"A","provider":"p","url":"http://x"}]}"#).is_err());
        assert!(Registry::from_json(r#"{"entries":[{"name":"A","provider":"p"},{"name":"A","provider":"q"}]}"#).is_err());
    }
}
