use std::net::Ipv4Addr;
use std::str::FromStr;

pub const MAX_EGRESS_ALLOW_ENTRIES: usize = 10;

pub fn parse_egress_allow(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn validate_egress_allow(entries: &[String]) -> Result<(), String> {
    if entries.len() > MAX_EGRESS_ALLOW_ENTRIES {
        return Err(format!(
            "at most {MAX_EGRESS_ALLOW_ENTRIES} entries are allowed (got {})",
            entries.len()
        ));
    }
    for entry in entries {
        if !is_valid_egress_entry(entry) {
            return Err(format!(
                "`{entry}` is not an IPv4 address, IPv4 CIDR, hostname, or wildcard hostname"
            ));
        }
    }
    Ok(())
}

pub fn is_valid_egress_entry(entry: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }
    if Ipv4Addr::from_str(entry).is_ok() {
        return true;
    }
    if entry.contains('/') {
        let mut parts = entry.splitn(2, '/');
        let addr = parts.next().unwrap_or_default();
        let prefix = parts.next().unwrap_or_default();
        let Ok(prefix) = prefix.parse::<u8>() else {
            return false;
        };
        return prefix <= 32 && Ipv4Addr::from_str(addr).is_ok();
    }
    if looks_like_ipv4(entry) {
        return false;
    }
    if let Some(host) = entry.strip_prefix("*.") {
        return is_valid_hostname(host);
    }
    is_valid_hostname(entry)
}

fn looks_like_ipv4(entry: &str) -> bool {
    let octets: Vec<&str> = entry.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| {
            !octet.is_empty() && octet.len() <= 3 && octet.bytes().all(|b| b.is_ascii_digit())
        })
}

fn is_valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.contains("..") {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}
