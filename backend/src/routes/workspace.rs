//! Workspace CRUD: the backend owns organizations, departments, personas,
//! groups, and preferences, so the client reads and mutates them through here
//! instead of holding its own authoritative copy.
//!
//! Conventions, uniform across every resource below:
//! - `POST` creates — 201 with the stored record. The body may carry an `id`;
//!   the server honours it when it's free and mints a fresh one otherwise, so a
//!   client can insert optimistically without waiting for a round-trip.
//! - `PATCH` merges a partial body (`{ ...existing, ...patch }`) — 200 with the
//!   stored record. Only the keys present are touched; `id` is always ignored.
//! - `DELETE` — 204, no body.
//! - Unknown id → 404. A patch that doesn't fit the resource's shape → 422. A
//!   request that collides with an invariant (a duplicate name, the last
//!   remaining user identity) → 409.
//!
//! Every route here resolves through [`CurrentAccount`], so the workspace a
//! request sees is the one belonging to its own token — the account *is* the
//! tenant boundary, and no id in a path or body can reach across it.

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::api_error::ApiError;
use crate::models::{
    Department, Group, Memory, MemoryInput, Organization, Persona, PromptLabel, PromptLabelInput,
    Settings,
};
use crate::state::{AppState, CurrentAccount};
use crate::workspace::{PatchError, PersonaError};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(list_orgs, create_org))
        .routes(routes!(get_org, update_org, delete_org))
        .routes(routes!(list_depts, create_dept))
        .routes(routes!(get_dept, update_dept, delete_dept))
        .routes(routes!(list_personas, create_persona))
        .routes(routes!(get_persona, update_persona, delete_persona))
        .routes(routes!(list_prompt_labels))
        .routes(routes!(set_prompt_label))
        .routes(routes!(list_memories, create_memory))
        .routes(routes!(delete_memory))
        .routes(routes!(list_groups, create_group))
        .routes(routes!(get_group, update_group, delete_group))
        .routes(routes!(get_preferences, update_preferences))
}

/// Turns a partial-update failure into its response. One place, so every
/// resource answers the same malformed patch the same way — the thing that
/// was inconsistent before this existed.
fn patch_error(error: PatchError, resource: &str, id: &str) -> ApiError {
    match error {
        PatchError::NotFound => ApiError::not_found(format!("no {resource} with id \"{id}\"")),
        PatchError::Invalid => ApiError::unprocessable(format!(
            "the patch does not fit a {resource}: a field has the wrong JSON type, or a \
             required field was set to null"
        )),
    }
}

// --- Organizations ----------------------------------------------------------

/// Every organization in the workspace.
#[utoipa::path(get, path = "/organizations", tag = "organizations",
    responses((status = 200, description = "Every organization", body = Vec<Organization>)))]
async fn list_orgs(s: CurrentAccount) -> Json<Vec<Organization>> {
    Json(s.workspace().organizations.clone())
}

/// One organization by id.
#[utoipa::path(get, path = "/organizations/{organizationId}", tag = "organizations",
    params(("organizationId" = String, Path, description = "The organization to read")),
    responses(
        (status = 200, description = "The organization", body = Organization),
        (status = 404, description = "Unknown organization")))]
async fn get_org(s: CurrentAccount, Path(id): Path<String>) -> Result<Json<Organization>, ApiError> {
    s.workspace()
        .organizations
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no organization with id \"{id}\"")))
}

/// Creates an organization.
#[utoipa::path(post, path = "/organizations", tag = "organizations",
    request_body = Organization,
    responses((status = 201, description = "The created organization", body = Organization)))]
async fn create_org(
    s: CurrentAccount,
    Json(body): Json<Organization>,
) -> (StatusCode, Json<Organization>) {
    let org = s.workspace().create_organization(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(org))
}

/// Merges a partial update onto an organization.
#[utoipa::path(patch, path = "/organizations/{organizationId}", tag = "organizations",
    params(("organizationId" = String, Path, description = "The organization to update")),
    request_body = Organization,
    responses(
        (status = 200, description = "The updated organization", body = Organization),
        (status = 404, description = "Unknown organization"),
        (status = 422, description = "Malformed patch")))]
async fn update_org(
    s: CurrentAccount,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Organization>, ApiError> {
    let updated = s.workspace().update_organization(&id, patch);
    if updated.is_ok() {
        s.persist_workspace();
    }
    updated.map(Json).map_err(|e| patch_error(e, "organization", &id))
}

/// Deletes an organization, cascading to its departments.
#[utoipa::path(delete, path = "/organizations/{organizationId}", tag = "organizations",
    params(("organizationId" = String, Path, description = "The organization to delete")),
    responses(
        (status = 204, description = "Deleted, cascading to its departments"),
        (status = 404, description = "Unknown organization")))]
async fn delete_org(s: CurrentAccount, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if s.workspace().delete_organization(&id) {
        s.persist_workspace();
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no organization with id \"{id}\"")))
    }
}

// --- Departments ------------------------------------------------------------

/// Every department in the workspace, across all organizations.
#[utoipa::path(get, path = "/departments", tag = "departments",
    responses((status = 200, description = "Every department, across all organizations", body = Vec<Department>)))]
async fn list_depts(s: CurrentAccount) -> Json<Vec<Department>> {
    Json(s.workspace().departments.clone())
}

/// One department by id.
#[utoipa::path(get, path = "/departments/{departmentId}", tag = "departments",
    params(("departmentId" = String, Path, description = "The department to read")),
    responses(
        (status = 200, description = "The department", body = Department),
        (status = 404, description = "Unknown department")))]
async fn get_dept(s: CurrentAccount, Path(id): Path<String>) -> Result<Json<Department>, ApiError> {
    s.workspace()
        .departments
        .iter()
        .find(|d| d.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no department with id \"{id}\"")))
}

/// Creates a department.
#[utoipa::path(post, path = "/departments", tag = "departments",
    request_body = Department,
    responses((status = 201, description = "The created department", body = Department)))]
async fn create_dept(
    s: CurrentAccount,
    Json(body): Json<Department>,
) -> (StatusCode, Json<Department>) {
    let dept = s.workspace().create_department(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(dept))
}

/// Merges a partial update onto a department.
#[utoipa::path(patch, path = "/departments/{departmentId}", tag = "departments",
    params(("departmentId" = String, Path, description = "The department to update")),
    request_body = Department,
    responses(
        (status = 200, description = "The updated department", body = Department),
        (status = 404, description = "Unknown department"),
        (status = 422, description = "Malformed patch")))]
async fn update_dept(
    s: CurrentAccount,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Department>, ApiError> {
    let updated = s.workspace().update_department(&id, patch);
    if updated.is_ok() {
        s.persist_workspace();
    }
    updated.map(Json).map_err(|e| patch_error(e, "department", &id))
}

/// Deletes a department.
#[utoipa::path(delete, path = "/departments/{departmentId}", tag = "departments",
    params(("departmentId" = String, Path, description = "The department to delete")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown department")))]
async fn delete_dept(s: CurrentAccount, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if s.workspace().delete_department(&id) {
        s.persist_workspace();
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no department with id \"{id}\"")))
    }
}

// --- Personas ---------------------------------------------------------------

/// Every persona in the workspace — the one user identity and every AI agent.
#[utoipa::path(get, path = "/personas", tag = "personas",
    responses((status = 200, description = "Every persona", body = Vec<Persona>)))]
async fn list_personas(s: CurrentAccount) -> Json<Vec<Persona>> {
    Json(s.workspace().personas.clone())
}

/// One persona by id.
#[utoipa::path(get, path = "/personas/{personaId}", tag = "personas",
    params(("personaId" = String, Path, description = "The persona to read")),
    responses(
        (status = 200, description = "The persona", body = Persona),
        (status = 404, description = "Unknown persona")))]
async fn get_persona(s: CurrentAccount, Path(id): Path<String>) -> Result<Json<Persona>, ApiError> {
    s.workspace()
        .personas
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no persona with id \"{id}\"")))
}

/// Creates a persona. Names are globally unique, and there is only ever one
/// user identity ("you"), so either collision is refused with 409.
#[utoipa::path(post, path = "/personas", tag = "personas",
    request_body = Persona,
    responses(
        (status = 201, description = "The created persona", body = Persona),
        (status = 409, description = "Name taken, or a second user identity")))]
async fn create_persona(
    s: CurrentAccount,
    Json(body): Json<Persona>,
) -> Result<(StatusCode, Json<Persona>), ApiError> {
    match s.workspace().create_persona(body) {
        Ok(persona) => {
            s.persist_workspace();
            Ok((StatusCode::CREATED, Json(persona)))
        }
        Err(e) => Err(persona_error(e, "")),
    }
}

/// Merges a partial update onto a persona. `promptHash` is recomputed from the
/// resulting prompt; any value sent for it is ignored.
#[utoipa::path(patch, path = "/personas/{personaId}", tag = "personas",
    params(("personaId" = String, Path, description = "The persona to update")),
    request_body = Persona,
    responses(
        (status = 200, description = "The updated persona", body = Persona),
        (status = 404, description = "Unknown persona"),
        (status = 409, description = "Name taken, or a second user identity"),
        (status = 422, description = "Malformed patch")))]
async fn update_persona(
    s: CurrentAccount,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Persona>, ApiError> {
    match s.workspace().update_persona(&id, patch) {
        Ok(persona) => {
            s.persist_workspace();
            Ok(Json(persona))
        }
        Err(e) => Err(persona_error(e, &id)),
    }
}

/// Turns a persona create/update failure into its response — the persona
/// analogue of [`patch_error`], with the two extra invariants personas carry.
fn persona_error(error: PersonaError, id: &str) -> ApiError {
    match error {
        PersonaError::NotFound => ApiError::not_found(format!("no persona with id \"{id}\"")),
        PersonaError::Invalid => ApiError::unprocessable(
            "the patch does not fit a Persona: a field has the wrong JSON type, or a \
             required field was set to null",
        ),
        PersonaError::NameTaken => ApiError::conflict(
            "name-taken",
            "another persona already uses this name; names are globally unique",
        ),
        PersonaError::UserExists => ApiError::conflict(
            "user-identity-exists",
            "there is already a user identity; a workspace has exactly one \"you\"",
        ),
    }
}

/// Deletes a persona. The last remaining user identity can't go — every group
/// still needs a "you".
#[utoipa::path(delete, path = "/personas/{personaId}", tag = "personas",
    params(("personaId" = String, Path, description = "The persona to delete")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown persona"),
        (status = 409, description = "Refused: the last user identity")))]
async fn delete_persona(s: CurrentAccount, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let mut ws = s.workspace();
    if !ws.personas.iter().any(|p| p.id == id) {
        return Err(ApiError::not_found(format!("no persona with id \"{id}\"")));
    }
    if ws.delete_persona(&id) {
        drop(ws);
        s.persist_workspace();
        Ok(StatusCode::NO_CONTENT)
    } else {
        // Found but refused: it's the last remaining user identity.
        Err(ApiError::conflict(
            "last-user-identity",
            "refusing to delete the last user identity; every group needs a \"you\"",
        ))
    }
}

// --- Prompt identity labels -------------------------------------------------

/// All user-assigned names for persona identity hashes. The memory UI reads this
/// to show a friendly label (e.g. "bar 版") next to a persona's prompt version.
#[utoipa::path(get, path = "/prompt-labels", tag = "personas",
    responses((status = 200, description = "Every named identity hash", body = Vec<PromptLabel>)))]
async fn list_prompt_labels(s: CurrentAccount) -> Json<Vec<PromptLabel>> {
    Json(s.workspace().prompt_labels())
}

/// Names an identity hash, or clears its name when the label is blank. Idempotent
/// (PUT), keyed by the full hash the client already holds from the persona.
#[utoipa::path(put, path = "/prompt-labels/{promptHash}", tag = "personas",
    params(("promptHash" = String, Path, description = "The full persona identity hash to name")),
    request_body = PromptLabelInput,
    responses((status = 200, description = "The stored label; empty when cleared", body = PromptLabel)))]
async fn set_prompt_label(
    s: CurrentAccount,
    Path(hash): Path<String>,
    Json(body): Json<PromptLabelInput>,
) -> Json<PromptLabel> {
    let label = s.workspace().set_prompt_label(&hash, &body.label);
    s.persist_workspace();
    Json(label)
}

// --- Persona memory ---------------------------------------------------------

/// Every memory a persona has accumulated, across all of its identity versions,
/// newest first. The memory-management UI groups the result by `promptHash`/label.
#[utoipa::path(get, path = "/personas/{personaId}/memories", tag = "personas",
    params(("personaId" = String, Path, description = "The persona whose memories to list")),
    responses(
        (status = 200, description = "Every memory, newest first", body = Vec<Memory>),
        (status = 404, description = "Unknown persona")))]
async fn list_memories(
    s: CurrentAccount,
    Path(persona_id): Path<String>,
) -> Result<Json<Vec<Memory>>, ApiError> {
    let ws = s.workspace();
    if !ws.personas.iter().any(|p| p.id == persona_id) {
        return Err(ApiError::not_found(format!("no persona with id \"{persona_id}\"")));
    }
    Ok(Json(ws.persona_memories(&persona_id)))
}

/// Writes a memory for a persona, tagged with its current identity hash.
#[utoipa::path(post, path = "/personas/{personaId}/memories", tag = "personas",
    params(("personaId" = String, Path, description = "The persona to remember this for")),
    request_body = MemoryInput,
    responses(
        (status = 201, description = "The created memory", body = Memory),
        (status = 404, description = "Unknown persona"),
        (status = 409, description = "The persona has no prompt to scope a memory to, or blank content")))]
async fn create_memory(
    s: CurrentAccount,
    Path(persona_id): Path<String>,
    Json(body): Json<MemoryInput>,
) -> Result<(StatusCode, Json<Memory>), ApiError> {
    let outcome = {
        let mut ws = s.workspace();
        if !ws.personas.iter().any(|p| p.id == persona_id) {
            None
        } else {
            Some(ws.add_memory(&persona_id, &body.content))
        }
    };
    match outcome {
        None => Err(ApiError::not_found(format!("no persona with id \"{persona_id}\""))),
        Some(None) => Err(ApiError::conflict(
            "memory-unscopable",
            "the persona has no system prompt to scope a memory to, or the content is blank",
        )),
        Some(Some(memory)) => {
            s.persist_workspace();
            Ok((StatusCode::CREATED, Json(memory)))
        }
    }
}

/// Deletes one of a persona's memories.
#[utoipa::path(delete, path = "/personas/{personaId}/memories/{memoryId}", tag = "personas",
    params(
        ("personaId" = String, Path, description = "The persona the memory belongs to"),
        ("memoryId" = String, Path, description = "The memory to delete")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown memory for that persona")))]
async fn delete_memory(
    s: CurrentAccount,
    Path((persona_id, memory_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    if s.workspace().delete_memory(&persona_id, &memory_id) {
        s.persist_workspace();
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "persona \"{persona_id}\" has no memory \"{memory_id}\""
        )))
    }
}

// --- Groups -----------------------------------------------------------------

/// Every chat group in the workspace.
#[utoipa::path(get, path = "/groups", tag = "groups",
    responses((status = 200, description = "Every group", body = Vec<Group>)))]
async fn list_groups(s: CurrentAccount) -> Json<Vec<Group>> {
    Json(s.workspace().groups.clone())
}

/// One group by id.
#[utoipa::path(get, path = "/groups/{groupId}", tag = "groups",
    params(("groupId" = String, Path, description = "The group to read")),
    responses(
        (status = 200, description = "The group", body = Group),
        (status = 404, description = "Unknown group")))]
async fn get_group(s: CurrentAccount, Path(id): Path<String>) -> Result<Json<Group>, ApiError> {
    s.workspace()
        .groups
        .iter()
        .find(|g| g.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no group with id \"{id}\"")))
}

/// Creates a group.
#[utoipa::path(post, path = "/groups", tag = "groups",
    request_body = Group,
    responses((status = 201, description = "The created group", body = Group)))]
async fn create_group(s: CurrentAccount, Json(body): Json<Group>) -> (StatusCode, Json<Group>) {
    let group = s.workspace().create_group(body);
    s.persist_workspace();
    (StatusCode::CREATED, Json(group))
}

/// Merges a partial update onto a group.
#[utoipa::path(patch, path = "/groups/{groupId}", tag = "groups",
    params(("groupId" = String, Path, description = "The group to update")),
    request_body = Group,
    responses(
        (status = 200, description = "The updated group", body = Group),
        (status = 404, description = "Unknown group"),
        (status = 422, description = "Malformed patch")))]
async fn update_group(
    s: CurrentAccount,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> Result<Json<Group>, ApiError> {
    let updated = s.workspace().update_group(&id, patch);
    if updated.is_ok() {
        s.persist_workspace();
    }
    updated.map(Json).map_err(|e| patch_error(e, "group", &id))
}

/// Deletes a group.
#[utoipa::path(delete, path = "/groups/{groupId}", tag = "groups",
    params(("groupId" = String, Path, description = "The group to delete")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown group")))]
async fn delete_group(s: CurrentAccount, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if s.workspace().delete_group(&id) {
        s.persist_workspace();
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no group with id \"{id}\"")))
    }
}

// --- Preferences ------------------------------------------------------------
//
// Named `/preferences`, not `/settings`: these are the signed-in person's own
// display choices, while `/llm/settings` (see `routes::llm`) is operator-level
// server configuration with a provider key behind it. Two things called
// "settings", one readable by anyone signed in and one admin-writable, is
// exactly the kind of near-miss that gets wired to the wrong endpoint.

/// The signed-in account's own display preferences.
#[utoipa::path(get, path = "/preferences", tag = "preferences",
    responses((status = 200, description = "This account's preferences", body = Settings)))]
async fn get_preferences(s: CurrentAccount) -> Json<Settings> {
    Json(s.workspace().settings.clone())
}

/// Merges a partial update onto the account's preferences.
#[utoipa::path(patch, path = "/preferences", tag = "preferences",
    request_body = Settings,
    responses(
        (status = 200, description = "The updated preferences", body = Settings),
        (status = 422, description = "Malformed patch")))]
async fn update_preferences(
    s: CurrentAccount,
    Json(patch): Json<Value>,
) -> Result<Json<Settings>, ApiError> {
    let updated = s.workspace().update_settings(patch);
    if updated.is_ok() {
        s.persist_workspace();
    }
    updated.map(Json).map_err(|e| patch_error(e, "Settings", ""))
}
