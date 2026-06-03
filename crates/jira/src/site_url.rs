//! Jira Cloud site-URL normalization (port of `jira/site-url.ts`). The stored
//! site URL builds every upstream request, so it is validated down to a single
//! canonical `https://<site>.atlassian.net` origin and nothing else (no path,
//! port, creds, query, or fragment) to prevent SSRF / credential-leaking
//! redirects.

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteUrlResult {
    Ok { origin: String },
    Err { reason: String },
}

/// `<sub>.atlassian.net` where `sub` is `[a-z0-9][a-z0-9-]*` (single label).
/// Host is already lowercased by `Url`, matching the JS regex's case-insensitive
/// intent.
fn is_jira_cloud_host(host: &str) -> bool {
    let Some(sub) = host.strip_suffix(".atlassian.net") else {
        return false;
    };
    if sub.is_empty() || sub.contains('.') {
        return false;
    }
    let mut chars = sub.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    sub.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn with_scheme(input: &str) -> String {
    // /^[a-z]+:\/\//i
    let has_scheme = input
        .find("://")
        .map(|i| i > 0 && input[..i].chars().all(|c| c.is_ascii_alphabetic()))
        .unwrap_or(false);
    if has_scheme {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

fn violation(url: &Url) -> Option<&'static str> {
    if url.scheme() != "https" {
        return Some("Site URL must use https.");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Some("Site URL must not contain credentials.");
    }
    if url.port().is_some() {
        return Some("Site URL must not specify a port.");
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Some("Site URL must not contain a path.");
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Some("Site URL must not contain a query or fragment.");
    }
    match url.host_str() {
        Some(host) if is_jira_cloud_host(host) => None,
        _ => Some("Site URL must be a *.atlassian.net Jira Cloud site."),
    }
}

pub fn normalize_site_url(input: &str) -> SiteUrlResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return SiteUrlResult::Err { reason: "Site URL is required.".to_string() };
    }
    let Ok(url) = Url::parse(&with_scheme(trimmed)) else {
        return SiteUrlResult::Err { reason: "Site URL is not valid.".to_string() };
    };
    match violation(&url) {
        None => SiteUrlResult::Ok { origin: url.origin().ascii_serialization() },
        Some(reason) => SiteUrlResult::Err { reason: reason.to_string() },
    }
}

/// True when `candidate_url` has the same origin as `site_origin` (port of
/// `isSameJiraOrigin`).
pub fn is_same_jira_origin(site_origin: &str, candidate_url: &str) -> bool {
    Url::parse(candidate_url)
        .map(|u| u.origin().ascii_serialization() == site_origin)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(origin: &str) -> SiteUrlResult {
        SiteUrlResult::Ok { origin: origin.to_string() }
    }

    fn is_ok(input: &str) -> bool {
        matches!(normalize_site_url(input), SiteUrlResult::Ok { .. })
    }

    #[test]
    fn accepts_bare_cloud_host() {
        assert_eq!(normalize_site_url("acme.atlassian.net"), ok("https://acme.atlassian.net"));
    }

    #[test]
    fn strips_trailing_slash() {
        assert_eq!(normalize_site_url("https://acme.atlassian.net/"), ok("https://acme.atlassian.net"));
    }

    #[test]
    fn lowercases_to_canonical_origin() {
        assert_eq!(normalize_site_url("https://ACME.Atlassian.NET"), ok("https://acme.atlassian.net"));
    }

    #[test]
    fn rejects_non_https() {
        assert!(!is_ok("http://acme.atlassian.net"));
    }

    #[test]
    fn rejects_path_beyond_root() {
        assert!(!is_ok("https://acme.atlassian.net/wiki"));
    }

    #[test]
    fn rejects_custom_port() {
        assert!(!is_ok("https://acme.atlassian.net:8443"));
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert!(!is_ok("https://user:pw@acme.atlassian.net"));
    }

    #[test]
    fn rejects_query_string() {
        assert!(!is_ok("https://acme.atlassian.net?x=1"));
    }

    #[test]
    fn rejects_fragment() {
        assert!(!is_ok("https://acme.atlassian.net#x"));
    }

    #[test]
    fn rejects_non_atlassian_host() {
        assert!(!is_ok("https://evil.com"));
    }

    #[test]
    fn rejects_deceptive_subdomain_host() {
        assert!(!is_ok("https://acme.atlassian.net.evil.com"));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(!is_ok("   "));
    }

    #[test]
    fn same_origin_true_for_same_site() {
        assert!(is_same_jira_origin(
            "https://acme.atlassian.net",
            "https://acme.atlassian.net/rest/api/3/myself"
        ));
    }

    #[test]
    fn same_origin_false_for_different_host() {
        assert!(!is_same_jira_origin("https://acme.atlassian.net", "https://evil.com/x"));
    }
}
