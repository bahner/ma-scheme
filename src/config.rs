#![allow(dead_code)]
/// Flat key-value configuration store for the zscheme runtime.
///
/// Keys are stored as dot-paths (e.g. `"my.aliases.sky"`).
/// Persisted to a YAML file at `$XDG_CONFIG_HOME/ma/zscheme-data.yaml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Flat key-value store with dot-path operations.
#[derive(Clone, Default)]
pub struct SchemeConfig {
    data: HashMap<String, String>,
}

impl SchemeConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from a YAML file. Creates an empty config if the file is absent.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::new(),
        };
        let map: HashMap<String, String> = serde_yaml::from_str(&text).unwrap_or_default();
        Self { data: map }
    }

    /// Persist to a YAML file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_yaml::to_string(&self.data)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Compute the default data file path.
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
        Ok(base
            .config_dir()
            .join("ma")
            .join("zscheme-data.yaml"))
    }

    /// Get a value at `path` (dot-path, leading `.` optional).
    pub fn get_str(&self, path: &str) -> Option<String> {
        let key = normalize_key(path);
        self.data.get(&key).cloned()
    }

    /// Set a value at `path`.
    pub fn set(&mut self, path: &str, value: &str) {
        self.data.insert(normalize_key(path), value.to_string());
    }

    /// Delete the subtree rooted at `path`.
    pub fn delete_subtree(&mut self, path: &str) {
        let key = normalize_key(path);
        // Remove exact key and all children (prefix + ".")
        let prefix = format!("{key}.");
        self.data
            .retain(|k, _| k != &key && !k.starts_with(&prefix));
    }

    /// List all `(key, value)` pairs that are direct or indirect children of `path`.
    pub fn list(&self, path: &str) -> Vec<(String, String)> {
        let key = normalize_key(path);
        let prefix = format!("{key}.");
        let mut pairs: Vec<(String, String)> = self
            .data
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix) || *k == &key)
            .map(|(k, v)| (format!(".{k}"), v.clone()))
            .collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        pairs
    }

    /// Resolve an alias name to its stored DID.
    pub fn resolve_alias(&self, name: &str) -> Option<String> {
        let bare = name.trim_start_matches('@');
        let key = format!("my.aliases.{bare}");
        self.data.get(&key).cloned()
    }

    /// Resolve an `@alias#fragment` or `did:ma:...#fragment` target to a full DID+fragment.
    pub fn resolve_target(&self, raw: &str) -> Result<String, String> {
        let raw = raw.trim_start_matches('@');
        if raw.starts_with("did:") {
            return Ok(raw.to_string());
        }
        if let Some((alias, fragment)) = raw.split_once('#') {
            if alias.is_empty() || fragment.is_empty() {
                return Err(format!("invalid target: {raw}"));
            }
            let did = self
                .resolve_alias(alias)
                .ok_or_else(|| format!("unknown alias: {alias}"))?;
            return Ok(format!("{did}#{fragment}"));
        }
        self.resolve_alias(raw)
            .ok_or_else(|| format!("unknown alias: {raw}"))
    }
}

/// Normalise a dot-path key: strip leading `.`, lowercase for lookups.
fn normalize_key(path: &str) -> String {
    path.trim_start_matches('.').to_string()
}

/// Returns true if the string looks like an IPFS CID or `did:ma:` link.
pub fn is_link_value(s: &str) -> bool {
    s.starts_with("did:ma:")
        || s.starts_with("bafy")
        || s.starts_with("bafk")
        || s.starts_with("bafz")
        || s.starts_with("bafei")
        || s.starts_with("Qm")
}

// Dot-path command parsing ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DotOp {
    Get,
    Set(String),
    Delete,
    Meta { verb: String, args: String },
}

/// Parse a dot-path command string into a path + operation.
///
/// Formats:
/// - `.my.path`          → Get
/// - `.my.path: value`   → Set("value")
/// - `.my.path:`         → Delete
/// - `.my.path!verb args`→ Meta { verb, args }
pub fn parse_dot_command(command: &str) -> Option<(String, DotOp)> {
    let s = command.trim().trim_start_matches('.');

    // Verb dispatch: .path!verb [args]
    if let Some(bang_idx) = s.find('!') {
        let path = s[..bang_idx].to_string();
        let rest = s[bang_idx + 1..].trim();
        let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
        return Some((
            path,
            DotOp::Meta {
                verb: verb.to_string(),
                args: args.to_string(),
            },
        ));
    }

    // Setter / Delete: find the FIRST colon (dot-paths never contain colons).
    // Formats: ".path: value" or ".path:" (delete)
    if let Some(colon_idx) = s.find(':') {
        let path = s[..colon_idx].to_string();
        let value = s[colon_idx + 1..].trim().to_string();
        if value.is_empty() {
            return Some((path, DotOp::Delete));
        } else {
            return Some((path, DotOp::Set(value)));
        }
    }

    // Get
    Some((s.to_string(), DotOp::Get))
}

/// Parse an actor command string into (target_with_fragment, verb, args).
///
/// Input is of the form:
///   `@alias#fragment:verb arg1 arg2`
///   `@alias#fragment arg1`
///   `@alias:verb arg1`
///   `did:ma:abc#fragment:verb arg1`
pub fn parse_actor_command(
    cmd: &str,
    config: &SchemeConfig,
) -> Result<(String, String, Vec<String>), String> {
    let (first_token, rest) = cmd
        .split_once(' ')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or_else(|| (cmd.to_string(), String::new()));

    // Resolve @alias → full DID (+fragment+verb part preserved)
    let resolved_first = if first_token.starts_with('@') {
        let bare = &first_token[1..]; // strip @
        resolve_actor_head(bare, config)?
    } else if first_token.starts_with("did:") {
        first_token.clone()
    } else {
        // Try treating as bare alias
        resolve_actor_head(&first_token, config)?
    };

    // At this point resolved_first is "did:ma:abc[#frag[:verb]]" or "did:ma:abc[:verb]"
    let (target_with_frag, verb) = split_resolved_did_verb(&resolved_first);

    let args: Vec<String> = if rest.is_empty() {
        vec![]
    } else {
        rest.split_whitespace().map(|s| s.to_string()).collect()
    };

    Ok((target_with_frag, verb, args))
}

/// Resolve an actor head (bare alias or alias+frag+verb) to its full form.
///
/// Input examples: `sky#house:enter`, `sky#ping`, `sky:ping`
/// Returns: `did:ma:abc#house:enter`, `did:ma:abc#ping`, `did:ma:abc:ping`
fn resolve_actor_head(head: &str, config: &SchemeConfig) -> Result<String, String> {
    if head.starts_with("did:") {
        return Ok(head.to_string());
    }
    // Find where the alias ends (at '#', ':', '.', or end)
    let alias_end = head
        .find(|c: char| c == '#' || c == ':' || c == '.')
        .unwrap_or(head.len());
    let alias = &head[..alias_end];
    let suffix = &head[alias_end..]; // "#frag:verb" or ":verb" or ""

    let did = config
        .resolve_alias(alias)
        .ok_or_else(|| format!("unknown alias: {alias}"))?;
    Ok(format!("{did}{suffix}"))
}

/// Split `did:ma:abc#frag:verb` or `did:ma:abc:verb` into (target_with_frag, verb).
///
/// - `did:ma:abc#house:enter` → (`did:ma:abc#house`, `enter`)
/// - `did:ma:abc#ping`        → (`did:ma:abc#ping`, ``)
/// - `did:ma:abc:ping`        → (`did:ma:abc`, `ping`)
/// - `did:ma:abc`             → (`did:ma:abc`, ``)
fn split_resolved_did_verb(s: &str) -> (String, String) {
    if let Some(hash_idx) = s.find('#') {
        let frag_and_maybe_verb = &s[hash_idx + 1..];
        if let Some(colon_idx) = frag_and_maybe_verb.find(':') {
            let frag = &frag_and_maybe_verb[..colon_idx];
            let verb = &frag_and_maybe_verb[colon_idx + 1..];
            let target = format!("{}#{}", &s[..hash_idx], frag);
            return (target, verb.to_string());
        }
        // No verb after fragment
        return (s.to_string(), String::new());
    }
    // No fragment — look for verb after the 3rd colon in `did:ma:abc:verb`
    let mut colon_count = 0;
    for (i, ch) in s.char_indices() {
        if ch == ':' {
            colon_count += 1;
            if colon_count == 3 {
                let (did_part, verb) = s.split_at(i);
                let verb = &verb[1..]; // strip leading ':'
                if verb.is_empty() {
                    return (did_part.to_string(), String::new());
                }
                return (did_part.to_string(), verb.to_string());
            }
        }
    }
    (s.to_string(), String::new())
}
