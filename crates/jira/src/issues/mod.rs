//! Jira issue data layer (port of `jira/issues/*` + the read half of
//! `action-runners.ts`): JQL builders, ADF flatten, field coercion, payload
//! mappers, and the search/dashboard/lookup fetchers. All fetchers take a
//! `&JiraClient`; the API layer resolves the connection and builds it.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

pub mod adf;
pub mod dashboard;
pub mod fields;
pub mod jql;
pub mod mappers;
pub mod reads;
pub mod search;
pub mod writes;

/// `encodeURIComponent` equivalent for path segments (issue keys, project keys,
/// numeric ids).
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub(crate) fn enc(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}
