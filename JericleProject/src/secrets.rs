//! API credentials, kept out of the repository entirely.
//!
//! Twelve Data is the first keyed upstream this project uses, so this module
//! exists to make a leak hard rather than merely unlikely:
//!
//! * The file lives **outside** the repo (`~/.opendash/secrets.toml`). `BA-Projects`
//!   is a public GitHub repository, so a key in a tracked file is a key anyone can
//!   clone, and the account can then be used up or its plan changed.
//! * File mode is checked on load; group- or world-readable is reported loudly.
//! * The key travels in an `Authorization` header, never a query string, so it
//!   cannot surface in an access log, a `Referer`, or an error that echoes the URL.
//! * `Debug` is hand-written to print `***`, so a stray `{:?}` cannot leak it, and
//!   there is deliberately no `Serialize` implementation.
//!
//! Providers are a map rather than a struct field per vendor, so adding a second
//! key later is a config change and not a code change. A misspelled provider name
//! resolves to `None` and surfaces as a clear message, never as a silent empty key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// `[providers.<name>] api_key = "..."` from the secrets file.
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderKey {
    pub api_key: String,
}

/// Loaded credentials. Missing file is not an error: it yields an empty set and the
/// features that need a key report themselves as unconfigured.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Secrets {
    pub providers: BTreeMap<String, ProviderKey>,
}

/// Redacted on purpose. `#[derive(Debug)]` here would print the key into any log
/// line, crash report or `dbg!` that happened to touch this value.
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field(
                "providers",
                &self.providers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl std::fmt::Debug for ProviderKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Length is not printed: it is a small, brute-forceable slice of the key.
        f.debug_struct("ProviderKey")
            .field("api_key", &"***")
            .finish()
    }
}

impl Secrets {
    /// Read the secrets file, or return an empty set when it does not exist.
    ///
    /// Resolution order mirrors `Config::load`: explicit path, `$OPENDASH_SECRETS`,
    /// then `$OPENDASH_DIR/secrets.toml`, then `~/.opendash/secrets.toml`.
    ///
    /// An **explicitly named** file that does not exist is *not* an invitation to
    /// fall back. Silently reading a different file than the one that was asked for
    /// is how a typo in `OPENDASH_SECRETS` turns into "the key changed" with no
    /// error anywhere — and on a machine with several accounts that is the
    /// difference between the right credentials and someone else's.
    pub fn load(explicit: Option<&str>) -> (Self, Option<PathBuf>) {
        if let Some(p) = explicit {
            let path = PathBuf::from(p);
            if !path.is_file() {
                eprintln!(
                    "opendash  secrets file {} does not exist; not falling back to another",
                    path.display()
                );
                return (Secrets::default(), None);
            }
            return Self::read(&path);
        }

        let candidates: Vec<PathBuf> = [
            std::env::var("OPENDASH_SECRETS").ok().map(PathBuf::from),
            std::env::var("OPENDASH_DIR").ok().map(|d| PathBuf::from(d).join("secrets.toml")),
            Some(default_path()),
        ]
        .into_iter()
        .flatten()
        .collect();

        let Some(path) = candidates.iter().find(|p| p.is_file()).cloned() else {
            return (Secrets::default(), None);
        };
        Self::read(&path)
    }

    fn read(path: &Path) -> (Self, Option<PathBuf>) {
        match read_secrets(path) {
            Ok((s, warn)) => {
                if let Some(w) = warn {
                    eprintln!("opendash  {w}");
                }
                (s, Some(path.to_path_buf()))
            }
            Err(e) => {
                // A malformed secrets file is worth shouting about: silently running
                // keyless looks identical to "the key is wrong".
                eprintln!("opendash  could not read {}: {e:#}", path.display());
                (Secrets::default(), Some(path.to_path_buf()))
            }
        }
    }

    /// The key for one provider, or `None` when unset or blank.
    pub fn api_key(&self, provider: &str) -> Option<&str> {
        let k = self.providers.get(provider)?.api_key.trim();
        if k.is_empty() {
            None
        } else {
            Some(k)
        }
    }

    pub fn has(&self, provider: &str) -> bool {
        self.api_key(provider).is_some()
    }

    /// Provider names that are present, for the startup banner.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

pub fn default_path() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h).join(".opendash").join("secrets.toml"),
        _ => PathBuf::from(".opendash").join("secrets.toml"),
    }
}

/// Parse the file and check its permissions. The warning is returned rather than
/// printed so the caller decides where it goes.
fn read_secrets(path: &Path) -> Result<(Secrets, Option<String>)> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let secrets: Secrets = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok((secrets, permission_warning(path)))
}

/// Warn when the file is readable by anyone but its owner.
///
/// A 0600 file is the only thing standing between a leaked key and a key anyone on
/// the machine can read, so a loose mode is a security problem, not a style one.
fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Some(format!(
                "{} is mode {:o}; it should be 600 so only you can read the API keys. \
                 Fix with: chmod 600 {}",
                path.display(),
                mode,
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("opendash-secrets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_a_provider_key() {
        let d = tmpdir("basic");
        let p = write(&d, "secrets.toml", "[providers.twelvedata]\napi_key = \"abc123\"\n");
        let (s, found) = Secrets::load(Some(p.to_str().unwrap()));
        assert_eq!(s.api_key("twelvedata"), Some("abc123"));
        assert_eq!(found.as_deref(), Some(p.as_path()));
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let d = tmpdir("missing");
        let (s, found) = Secrets::load(Some(d.join("nope.toml").to_str().unwrap()));
        assert!(!s.has("twelvedata"));
        assert!(found.is_none());
    }

    /// A mistyped `OPENDASH_SECRETS` must not quietly load some *other* file's
    /// keys. The live `~/.opendash/secrets.toml` exists on this machine, so a
    /// silent fallback would make the test above pass while the daemon ran with
    /// real credentials it was never asked to use.
    #[test]
    fn an_explicit_missing_path_does_not_fall_back_to_the_default() {
        let d = tmpdir("nofallback");
        let (s, found) = Secrets::load(Some(d.join("absent.toml").to_str().unwrap()));
        assert!(!s.has("twelvedata"), "must not have loaded the real secrets file");
        assert!(found.is_none());
    }

    /// The whole point of the hand-written Debug: a stray `{:?}` must not print
    /// the key, and must not print its length either.
    #[test]
    fn debug_redacts_the_key() {
        let s: Secrets = toml::from_str("[providers.twelvedata]\napi_key = \"SUPERSECRET\"\n").unwrap();
        let out = format!("{s:?} {s:?}");
        assert!(!out.contains("SUPERSECRET"), "Debug leaked the key: {out}");
        assert!(out.contains("twelvedata"), "Debug should still name the provider");
        assert!(!out.contains("10"), "Debug leaked the key length: {out}");

        let k = &s.providers["twelvedata"];
        assert!(!format!("{k:?}").contains("SUPERSECRET"));
    }

    #[test]
    fn blank_key_reads_as_absent() {
        let s: Secrets = toml::from_str("[providers.twelvedata]\napi_key = \"  \"\n").unwrap();
        assert_eq!(s.api_key("twelvedata"), None);
        assert!(!s.has("twelvedata"));
    }

    #[test]
    fn unknown_provider_resolves_to_none_not_a_wrong_key() {
        let s: Secrets = toml::from_str("[providers.twelvedata]\napi_key = \"k\"\n").unwrap();
        assert_eq!(s.api_key("twelve_data"), None, "typo must not match");
        assert_eq!(s.api_key("TwelveData"), None, "lookup is case sensitive");
    }

    /// A second provider needs no code change, which is the point of the map.
    #[test]
    fn a_second_provider_needs_no_code_change() {
        let s: Secrets = toml::from_str(
            "[providers.twelvedata]\napi_key = \"one\"\n\n[providers.somefuture]\napi_key = \"two\"\n",
        )
        .unwrap();
        assert_eq!(s.api_key("twelvedata"), Some("one"));
        assert_eq!(s.api_key("somefuture"), Some("two"));
        assert_eq!(s.provider_names(), vec!["somefuture", "twelvedata"]);
    }

    #[test]
    fn a_typo_in_the_file_is_a_parse_error_not_a_silent_miss() {
        // deny_unknown_fields: a misspelled field would otherwise leave the key
        // empty and present a "no key configured" state that looks intentional.
        let bad = toml::from_str::<Secrets>("[providers.twelvedata]\napikey = \"x\"\n");
        assert!(bad.is_err(), "misspelled field should not parse");
    }

    #[cfg(unix)]
    #[test]
    fn a_loose_file_mode_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("perms");
        let p = write(&d, "secrets.toml", "[providers.twelvedata]\napi_key = \"k\"\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let warn = permission_warning(&p).expect("0644 should warn");
        assert!(warn.contains("chmod 600"), "warning should be actionable: {warn}");

        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(permission_warning(&p).is_none(), "0600 should be silent");
    }
}
