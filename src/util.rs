use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 32 random bytes, hex-encoded.
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(data.as_ref()))
}

/// Generates a project API key, returning `(key, hash, display_prefix)`.
pub fn new_api_key() -> (String, String, String) {
    let key = format!("dk_{}", random_token());
    let hash = sha256_hex(&key);
    let prefix = key[..10].to_string();
    (key, hash, prefix)
}

/// Folds common Latin accented letters to ASCII ("é" -> "e") for slugs.
fn fold_accent(c: char) -> char {
    match c {
        'à'..='å' | 'À'..='Å' => 'a',
        'ç' | 'Ç' => 'c',
        'è'..='ë' | 'È'..='Ë' => 'e',
        'ì'..='ï' | 'Ì'..='Ï' => 'i',
        'ñ' | 'Ñ' => 'n',
        'ò'..='ö' | 'ø' | 'Ò'..='Ö' | 'Ø' => 'o',
        'ù'..='ü' | 'Ù'..='Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        _ => c,
    }
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().map(fold_accent) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    out.chars().take(64).collect::<String>().trim_end_matches('-').to_string()
}

pub fn valid_slug(s: &str) -> bool {
    (2..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// Accepts only http(s) URLs so user content can never produce `javascript:` links.
pub fn clean_url(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() > 300 || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    (s.starts_with("https://") || s.starts_with("http://")).then(|| s.to_string())
}

/// Formats an RFC 3339 timestamp for `<time datetime>`.
pub fn iso_time(ts: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default()
}

/// Human-readable date (UTC) used as no-JS fallback for relative times.
pub fn human_date(ts: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(time::macros::format_description!("[year]-[month]-[day] [hour]:[minute] UTC")).ok())
        .unwrap_or_default()
}

pub fn fmt_pct(v: f64) -> String {
    if v >= 100.0 {
        "100%".into()
    } else if v > 0.0 && v < 0.01 {
        "<0.01%".into()
    } else {
        format!("{v:.2}%")
    }
}

pub fn fmt_delta(v: f64) -> String {
    if v.abs() < 0.005 {
        "±0%".into()
    } else if v > 0.0 {
        format!("+{v:.2}%")
    } else {
        format!("{v:.2}%")
    }
}

pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("The Legend of Zelda: Ocarina of Time!"), "the-legend-of-zelda-ocarina-of-time");
        assert_eq!(slugify("Pokémon Émerald"), "pokemon-emerald");
        assert!(valid_slug("sm64"));
        assert!(!valid_slug("-bad"));
        assert_eq!(fmt_count(1234567), "1,234,567");
    }
}
