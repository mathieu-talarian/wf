//! Tiny shared response DTOs used only to document the `{ ok }` /
//! `{ disconnected }` JSON envelopes in the OpenAPI spec (Phase 5a).

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct OkResponse {
    pub ok: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DisconnectedResponse {
    pub disconnected: bool,
}
