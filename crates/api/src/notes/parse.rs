//! Note-body parsing: `⏰ <when>` reminders and `[[TICKET-KEY]]` wiki-links.
//!
//! Reminder identity is FNV-1a64(`ticket_key`, `body`, `due_at`) rendered as
//! hex — stable across saves, so done/snoozed state survives edits that don't
//! touch the reminder line. `<when>` accepts: ISO date/datetime, `today` /
//! `tomorrow`, weekday names (next occurrence), each with an optional
//! `HH:MM` (default 09:00), or a bare `HH:MM` (today, rolling to tomorrow if
//! already past) — all interpreted in the caller's IANA timezone.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;

pub struct ParsedReminder {
    pub id: String,
    /// The task/line text with the `⏰ <when>` token removed.
    pub body: String,
    pub due_at: DateTime<Utc>,
}

pub struct ParsedLink {
    pub to_ticket: String,
    /// The whole line containing the link (the backlink snippet).
    pub snippet: String,
}

fn wiki_link_regex() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\[\[([A-Z][A-Z0-9]+-\d+)\]\]").expect("static regex compiles")
    })
}

fn clock_regex() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // `⏰` then everything up to a separator (`·`, `,`, `;`) or EOL.
    RE.get_or_init(|| regex::Regex::new(r"⏰\s*([^·,;\n]*)").expect("static regex compiles"))
}

/// FNV-1a 64 — tiny, deterministic, dependency-free; collisions are acceptable
/// (worst case two identical-looking reminders merge).
fn fnv64(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff; // separator so ("a","b") != ("ab","")
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Strips list/task markup so reminder bodies read naturally in the inbox.
fn strip_task_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    for prefix in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest;
        }
    }
    trimmed
}

pub fn parse_links(ticket_key: &str, body: &str) -> Vec<(String, String)> {
    let mut links: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        for capture in wiki_link_regex().captures_iter(line) {
            let to = capture[1].to_string();
            if to != ticket_key && !links.iter().any(|(t, _)| t == &to) {
                links.push((to, line.trim().to_string()));
            }
        }
    }
    links
}

pub fn parse_reminders(
    ticket_key: &str,
    body: &str,
    tz: Tz,
    now: DateTime<Utc>,
) -> Vec<ParsedReminder> {
    let mut reminders = Vec::new();
    for line in body.lines() {
        let Some(capture) = clock_regex().captures(line) else { continue };
        let expr = capture[1].trim();
        let Some(due_at) = parse_when(expr, tz, now) else { continue };
        let without_token = clock_regex().replace(line, "").to_string();
        let text = strip_task_prefix(&without_token).trim().to_string();
        let due_iso = due_at.to_rfc3339();
        reminders.push(ParsedReminder {
            id: fnv64(&[ticket_key, &text, &due_iso]),
            body: text,
            due_at,
        });
    }
    reminders
}

const DEFAULT_TIME: (u32, u32) = (9, 0);

/// Parses the `<when>` expression. Returns `None` for unparseable input — the
/// token then simply doesn't create a reminder (never an error).
fn parse_when(expr: &str, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let tokens: Vec<&str> = expr.split_whitespace().collect();
    let local_now = now.with_timezone(&tz);
    let today = local_now.date_naive();

    let (date, time, consumed_time) = match tokens.first() {
        None => return None,
        Some(&first) => {
            let lower = first.to_lowercase();
            if let Ok(date) = NaiveDate::parse_from_str(first, "%Y-%m-%d") {
                (Some(date), tokens.get(1).and_then(|t| parse_time(t)), true)
            } else if let Some(dt) = parse_iso_datetime(first) {
                return local_to_utc(tz, dt.0, dt.1);
            } else if lower == "today" {
                (Some(today), tokens.get(1).and_then(|t| parse_time(t)), true)
            } else if lower == "tomorrow" {
                (Some(today + Duration::days(1)), tokens.get(1).and_then(|t| parse_time(t)), true)
            } else if let Some(weekday) = parse_weekday(&lower) {
                let mut date = today + Duration::days(1);
                while date.weekday() != weekday {
                    date += Duration::days(1);
                }
                (Some(date), tokens.get(1).and_then(|t| parse_time(t)), true)
            } else if let Some(time) = parse_time(first) {
                // Bare HH:MM: today, else tomorrow.
                let candidate = local_to_utc(tz, today, time)?;
                let due = if candidate <= now {
                    local_to_utc(tz, today + Duration::days(1), time)?
                } else {
                    candidate
                };
                return Some(due);
            } else {
                return None;
            }
        }
    };
    let _ = consumed_time;
    let time = time
        .unwrap_or_else(|| NaiveTime::from_hms_opt(DEFAULT_TIME.0, DEFAULT_TIME.1, 0).expect("valid"));
    local_to_utc(tz, date?, time)
}

fn parse_iso_datetime(token: &str) -> Option<(NaiveDate, NaiveTime)> {
    let dt = chrono::NaiveDateTime::parse_from_str(token, "%Y-%m-%dT%H:%M").ok()?;
    Some((dt.date(), dt.time()))
}

fn parse_time(token: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(token, "%H:%M").ok()
}

fn parse_weekday(token: &str) -> Option<Weekday> {
    match &token[..token.len().min(3)] {
        "mon" => Some(Weekday::Mon),
        "tue" => Some(Weekday::Tue),
        "wed" => Some(Weekday::Wed),
        "thu" => Some(Weekday::Thu),
        "fri" => Some(Weekday::Fri),
        "sat" => Some(Weekday::Sat),
        "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

fn local_to_utc(tz: Tz, date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        // 2026-06-11 is a Thursday; 12:00 UTC.
        Utc.with_ymd_and_hms(2026, 6, 11, 12, 0, 0).unwrap()
    }

    const TZ: Tz = chrono_tz::Europe::Paris;

    #[test]
    fn parses_tomorrow_with_time() {
        let parsed = parse_reminders("GPT-1", "- [ ] ping QA ⏰ tomorrow 09:00", TZ, now());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].body, "ping QA");
        // 09:00 Paris (CEST, +2) == 07:00 UTC.
        assert_eq!(parsed[0].due_at, Utc.with_ymd_and_hms(2026, 6, 12, 7, 0, 0).unwrap());
    }

    #[test]
    fn parses_weekday_iso_and_bare_time() {
        let parsed = parse_reminders(
            "GPT-1",
            "⏰ fri 14:00 check deploy\n⏰ 2026-07-01 release\nremind ⏰ 10:00",
            TZ,
            now(),
        );
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].due_at, Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap());
        assert_eq!(parsed[1].due_at, Utc.with_ymd_and_hms(2026, 7, 1, 7, 0, 0).unwrap());
        // 10:00 Paris is in the past at 12:00 UTC (14:00 local) → tomorrow.
        assert_eq!(parsed[2].due_at, Utc.with_ymd_and_hms(2026, 6, 12, 8, 0, 0).unwrap());
    }

    #[test]
    fn id_stable_and_unparseable_skipped() {
        let a = parse_reminders("GPT-1", "- [ ] x ⏰ tomorrow", TZ, now());
        let b = parse_reminders("GPT-1", "- [ ] x ⏰ tomorrow\nnew unrelated line", TZ, now());
        assert_eq!(a[0].id, b[0].id);
        assert!(parse_reminders("GPT-1", "⏰ whenever", TZ, now()).is_empty());
    }

    #[test]
    fn links_dedupe_and_skip_self() {
        let links = parse_links("GPT-1", "see [[GPT-2]] and [[GPT-2]]\nself [[GPT-1]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, "GPT-2");
        assert_eq!(links[0].1, "see [[GPT-2]] and [[GPT-2]]");
    }
}
