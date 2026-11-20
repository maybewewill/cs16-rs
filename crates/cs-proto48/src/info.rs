//! GoldSrc `\key\value\...` info-string parser and serializer (`engine/info.cpp`).

/// Ordered key-value map encoded as `\key1\val1\key2\val2`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InfoString {
    entries: Vec<(String, String)>,
}

impl InfoString {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a GoldSrc info string (`\k1\v1\k2\v2`).
    pub fn parse(raw: &str) -> Self {
        let mut entries = Vec::new();
        let trimmed = raw.strip_prefix('\\').unwrap_or(raw);
        let mut parts = trimmed.split('\\');
        while let Some(k) = parts.next() {
            if k.is_empty() {
                break;
            }
            let v = parts.next().unwrap_or("");
            entries.push((k.to_string(), v.to_string()));
        }
        Self { entries }
    }

    /// GoldSrc `Info_IsValid`: non-empty, even number of key/value pairs, no quotes.
    pub fn is_valid(raw: &str) -> bool {
        if raw.is_empty() || raw.contains('"') {
            return false;
        }
        let s = raw.strip_prefix('\\').unwrap_or(raw);
        let mut count = 0usize;
        for part in s.split('\\') {
            if count.is_multiple_of(2) && part.is_empty() {
                return false;
            }
            count += 1;
        }
        count >= 2 && count.is_multiple_of(2)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if value.is_empty() {
            self.remove(&key);
            return;
        }
        if let Some((_, existing)) = self
            .entries
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&key))
        {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
    }

    /// Remove private keys starting with `prefix` (e.g. `Info_RemovePrefixedKeys(info, '_')`).
    pub fn remove_prefixed(&mut self, prefix: char) {
        self.entries.retain(|(k, _)| !k.starts_with(prefix));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn to_wire_string(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.entries {
            out.push('\\');
            out.push_str(k);
            out.push('\\');
            out.push_str(v);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_validates_and_roundtrips_goldsrc_info_strings() {
        let raw = r"\prot\3\unique\-1\raw\steam\cdkey\0123456789abcdef0123456789abcdef";
        assert!(InfoString::is_valid(raw));
        let mut info = InfoString::parse(raw);
        assert_eq!(info.get("prot"), Some("3"));
        assert_eq!(info.get("raw"), Some("steam"));
        assert_eq!(info.to_wire_string(), raw);

        info.set("_cl_autowepswitch", "1");
        info.remove_prefixed('_');
        assert_eq!(info.get("_cl_autowepswitch"), None);
    }
}
