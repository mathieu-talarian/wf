//! AI assists (`ai` tag): per-user toggles, the OpenAI Chat Completions client, and
//! the draft endpoints. The morning brief plugs into `hub::inbox`.

pub mod openai;
pub mod routes;
pub mod settings;
