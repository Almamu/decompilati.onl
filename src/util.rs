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

/// Parses Unix seconds or an RFC 3339 date-time.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    s.parse::<i64>().ok().or_else(|| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .ok()
            .map(|t| t.unix_timestamp())
    })
}

/// First path segments used by the site itself. decomp.dev-style `/{owner}/{repo}` URLs are
/// redirected to projects, so GitHub accounts with these names can't use the site.
pub const RESERVED_NAMES: &[&str] = &[
    "p", "project", "projects", "platform", "platforms", "docs", "doc", "dashboard", "admin", "auth", "api",
    "static", "uploads", "badge", "badges", "feed.xml", "robots.txt", "sitemap.xml", "favicon.ico", "login",
    "logout", "new", "about", "help", "settings", "user", "users",
];

pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(name))
}

/// Normalises a Discord invite (`discord.gg/abc`, `https://discord.com/invite/abc`, …) to
/// `https://discord.gg/abc`. Empty input is `Ok(None)`; anything that isn't an invite is an error,
/// so the field can't be used to link elsewhere.
pub fn normalize_discord_invite(input: &str) -> Result<Option<String>, &'static str> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let code = ["discord.gg/", "discord.com/invite/", "discordapp.com/invite/"]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .map(|c| c.trim_end_matches('/'))
        .ok_or("The Discord link must be an invite, like https://discord.gg/abc123.")?;
    if !(2..=64).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("That Discord invite code doesn't look right.");
    }
    Ok(Some(format!("https://discord.gg/{code}")))
}

#[cfg(test)]
mod discord_tests {
    use super::normalize_discord_invite as n;

    #[test]
    fn invites() {
        assert_eq!(n(""), Ok(None));
        assert_eq!(n("discord.gg/abc123"), Ok(Some("https://discord.gg/abc123".into())));
        assert_eq!(n("https://discord.com/invite/Abc-123/"), Ok(Some("https://discord.gg/Abc-123".into())));
        assert!(n("https://evil.example/discord.gg/x").is_err());
        assert!(n("https://discord.gg/a b").is_err());
        assert!(n("javascript:alert(1)").is_err());
    }
}

/// Checks a repository URL outside GitHub and returns `(display, url)`: the display form is
/// `host/path` (e.g. `gitlab.com/owner/game`). GitHub URLs are refused: those are picked
/// from the user's own repositories, where push access is verified.
pub fn normalize_external_repo(input: &str) -> Result<(String, String), &'static str> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let rest = s.strip_prefix("https://").ok_or("The repository URL must start with https://")?;
    if s.len() > 300 || rest.chars().any(|c| c.is_whitespace() || c.is_control() || "\"'<>\\?#@".contains(c)) {
        return Err("That doesn't look like a repository URL.");
    }
    let (host, path) = rest.split_once('/').ok_or("Include the path to the repository, not just the site.")?;
    let host = host.to_ascii_lowercase();
    let valid_host = host.contains('.')
        && host.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if !valid_host || path.is_empty() || path.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return Err("That doesn't look like a repository URL.");
    }
    if host == "github.com" || host == "www.github.com" {
        return Err("For GitHub repositories, choose \"GitHub\" and pick it from the list.");
    }
    Ok((format!("{host}/{path}"), format!("https://{host}/{path}")))
}

#[cfg(test)]
mod external_repo_tests {
    use super::normalize_external_repo as n;

    #[test]
    fn urls() {
        assert_eq!(
            n("https://GitLab.com/owner/game.git/"),
            Ok(("gitlab.com/owner/game".into(), "https://gitlab.com/owner/game".into()))
        );
        assert!(n("https://github.com/a/b").is_err());
        assert!(n("http://gitlab.com/a/b").is_err());
        assert!(n("https://gitlab.com").is_err());
        assert!(n("https://gitlab.com/a/../b").is_err());
        assert!(n("javascript:alert(1)").is_err());
        assert!(n("https://evil\".com/x").is_err());
    }
}
