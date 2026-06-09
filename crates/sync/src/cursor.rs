//! Compound poll cursors (A1 spec §6.2), JSON-encoded into
//! `sync_state."cursor"`. GitHub compares `(updated_at, id)`; Jira compares
//! `(updated-as-instant, issue_key)` — Jira timestamps carry a UTC offset
//! (`2026-06-02T11:00:00.000+0200`), so ordering parses them rather than
//! comparing strings.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubCursor {
    pub updated_at: DateTime<Utc>,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraCursor {
    /// Raw Jira `updated` string (round-trips exactly; parsed for ordering).
    pub updated: String,
    pub issue_key: String,
}

/// Parses a stored cursor; `None`/garbage → `None` (treated as baseline).
pub fn parse<T: serde::de::DeserializeOwned>(raw: Option<&str>) -> Option<T> {
    raw.and_then(|s| serde_json::from_str(s).ok())
}

/// Encodes a cursor for storage.
pub fn encode<T: Serialize>(cursor: &T) -> String {
    serde_json::to_string(cursor).expect("cursor serializes")
}

impl GithubCursor {
    /// Is `(updated_at, id)` strictly after this cursor?
    pub fn is_before(&self, updated_at: DateTime<Utc>, id: i64) -> bool {
        updated_at > self.updated_at || (updated_at == self.updated_at && id > self.id)
    }
}

/// Parses a Jira timestamp (`%Y-%m-%dT%H:%M:%S%.3f%z`).
pub fn parse_jira_ts(raw: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.3f%z").ok()
}

impl JiraCursor {
    /// Is `(updated, key)` strictly after this cursor? Unparseable timestamps
    /// order as "not after" (skipped; dedup keeps this safe).
    pub fn is_before(&self, updated: &str, key: &str) -> bool {
        let (Some(a), Some(b)) = (parse_jira_ts(&self.updated), parse_jira_ts(updated)) else {
            return false;
        };
        b > a || (b == a && key > self.issue_key.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn gh(ts: i64, id: i64) -> GithubCursor {
        GithubCursor { updated_at: Utc.timestamp_opt(ts, 0).unwrap(), id }
    }

    #[test]
    fn github_ordering_with_tiebreak() {
        let cur = gh(100, 5);
        assert!(cur.is_before(Utc.timestamp_opt(101, 0).unwrap(), 1));
        assert!(cur.is_before(Utc.timestamp_opt(100, 0).unwrap(), 6));
        assert!(!cur.is_before(Utc.timestamp_opt(100, 0).unwrap(), 5));
        assert!(!cur.is_before(Utc.timestamp_opt(99, 0).unwrap(), 9));
    }

    #[test]
    fn jira_ordering_across_offsets() {
        let cur = JiraCursor {
            updated: "2026-06-02T11:00:00.000+0200".into(), // 09:00Z
            issue_key: "PROJ-1".into(),
        };
        assert!(cur.is_before("2026-06-02T10:00:00.000+0000", "PROJ-1")); // 10:00Z
        assert!(!cur.is_before("2026-06-02T08:00:00.000+0000", "PROJ-9")); // 08:00Z
        assert!(cur.is_before("2026-06-02T11:00:00.000+0200", "PROJ-2")); // tie → key
        assert!(!cur.is_before("garbage", "PROJ-2"));
    }

    #[test]
    fn round_trips_through_json() {
        let cur = gh(100, 5);
        let parsed: GithubCursor = parse(Some(&encode(&cur))).unwrap();
        assert_eq!(parsed, cur);
        assert_eq!(parse::<GithubCursor>(Some("not json")), None);
        assert_eq!(parse::<GithubCursor>(None), None);
    }
}
