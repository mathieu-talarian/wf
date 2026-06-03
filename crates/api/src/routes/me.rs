//! `GET /api/me` (migration plan §14.1): verifies the Supabase JWT (via the
//! `AuthUser` extractor), upserts the user row from the claims, and returns it.
//! Port of `routes-me.ts`.

use actix_web::{web, HttpResponse};
use chrono::SecondsFormat;
use serde::Serialize;
use wf_db::repositories::users;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    id: String,
    email: String,
    name: Option<String>,
    avatar_url: Option<String>,
    created_at: String,
    updated_at: String,
}

async fn me(state: web::Data<AppState>, user: AuthUser) -> Result<HttpResponse, AppError> {
    let row = users::upsert_from_auth(&state.db, &user.0).await?;
    let resp = MeResponse {
        id: row.id.to_string(),
        email: row.email,
        name: row.name,
        avatar_url: row.avatar_url,
        // `Date#toISOString()` parity: UTC, millisecond precision, `Z`.
        created_at: row
            .created_at
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        updated_at: row
            .updated_at
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    Ok(HttpResponse::Ok().json(resp))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/me", web::get().to(me));
}
