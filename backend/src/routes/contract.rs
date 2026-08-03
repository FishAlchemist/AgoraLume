//! Document-wide passes that keep the generated OpenAPI contract consistent.
//!
//! Facts that hold for *every* endpoint don't belong in every endpoint's
//! annotation — repeated forty times, one of them eventually gets missed, and
//! the document starts lying about a route nobody re-read. These
//! [`utoipa::Modify`] passes state each such fact once and apply it
//! mechanically to the finished document, so a new handler inherits it by
//! existing rather than by remembering.

use utoipa::Modify;
use utoipa::openapi::path::Operation;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{OpenApi, Ref, Response, ResponseBuilder};

/// The name the `bearerAuth` scheme is registered under, and referenced by in
/// every operation's `security` requirement.
pub const BEARER_SCHEME: &str = "bearerAuth";

/// Applies the passes that need the *finished* document.
///
/// [`SecurityAddon`] can run as a derive-level modifier because it only touches
/// document-level components, but the two below walk every operation — and at
/// derive time `ApiDoc` has no operations at all: the paths arrive later, when
/// the handler routers are merged and nested. Running them there would have
/// silently done nothing, so they run here instead, on the assembled document
/// [`crate::routes::openapi`] hands out.
pub fn finalize(openapi: &mut OpenApi) {
    PatchBodyAddon.modify(openapi);
    AuthResponsesAddon.modify(openapi);
    ProblemJsonAddon.modify(openapi);
}

/// Registers the bearer scheme the whole API is protected by.
///
/// Without this the document never mentioned authentication at all: a
/// generated client had no idea a token existed, and a reader couldn't tell a
/// public endpoint from a protected one. The requirement itself is declared
/// globally on `ApiDoc`; the four genuinely public operations opt out with
/// `security(())`.
pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut OpenApi) {
        // `components` is already populated by the handler-contributed schemas.
        // Build it only if it somehow isn't — replacing it wholesale (as the
        // upstream example does) would drop every schema in the document.
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            BEARER_SCHEME,
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "Access token from `POST /auth/login`. Opaque; does not survive a \
                         server restart. Waived when `GET /meta` reports \
                         `authRequired: false`.",
                    ))
                    .build(),
            ),
        );
    }
}

/// Adds the two authentication failures to every operation that requires a
/// token, so no protected route can silently omit them.
///
/// An operation is protected exactly when it hasn't opted out with an empty
/// `security` requirement, which is the same rule the server applies: the
/// extractors ([`crate::state::CurrentAccount`] and friends) run on everything
/// except the four public routes.
///
/// Both statuses apply to every protected route, including the ones that only
/// look account-shaped: a request with no usable token is a 401, and a request
/// whose token authenticates *some* subject that may not act here is a 403 —
/// an admin token on a per-account route, or an account token on an admin one.
pub struct AuthResponsesAddon;

impl Modify for AuthResponsesAddon {
    fn modify(&self, openapi: &mut OpenApi) {
        for operation in operations(openapi) {
            // `None` inherits the document-wide requirement; `Some([])` is the
            // explicit opt-out the public routes declare. A requirement that is
            // present and non-empty is protected in its own right.
            let protected = match &operation.security {
                None => true,
                Some(requirements) => requirements.iter().any(is_real_requirement),
            };
            if !protected {
                continue;
            }
            // Kept to one line each: these are copied onto every protected
            // operation, so a paragraph here is a paragraph forty times over in
            // a generated file that gets diffed on every regeneration. The
            // reasoning belongs in this module's docs, not in the output.
            let responses = &mut operation.responses.responses;
            responses
                .entry("401".to_string())
                .or_insert_with(|| problem_response("Not signed in").into());
            responses
                .entry("403".to_string())
                .or_insert_with(|| problem_response("Not permitted for this identity").into());
        }
    }
}

/// Points every `PATCH` body at a schema whose fields are all optional.
///
/// The handlers take an arbitrary JSON object and merge it key-by-key, but each
/// operation was annotated with the *whole* resource — so the document claimed
/// that `PATCH /groups/{groupId}` required `name`, `personaIds` and
/// `selfPersonaId`, when sending only `{"name": "…"}` is the entire point. A
/// generated client believing the document would have made partial updates
/// impossible to express.
///
/// The relaxed schema is derived from the real one here rather than hand-written
/// as a second struct: a mirror type would drift the first time a field was
/// added to the original, which is the failure this whole module exists to
/// prevent. Schemas already shaped as partial updates — anything named `…Patch`
/// or `…Request` — are left alone.
pub struct PatchBodyAddon;

impl Modify for PatchBodyAddon {
    fn modify(&self, openapi: &mut OpenApi) {
        // Three passes because each borrows a different part of the document:
        // read the names off the operations, derive the schemas, then repoint
        // the operations at them.
        let targets: Vec<String> = openapi
            .paths
            .paths
            .values()
            .filter_map(|item| item.patch.as_ref())
            .filter_map(patch_body_schema_name)
            .filter(|name| !name.ends_with("Patch") && !name.ends_with("Request"))
            .collect();

        let Some(components) = openapi.components.as_mut() else {
            return;
        };
        let mut derived: Vec<(String, String)> = Vec::new();
        for name in targets {
            let patch_name = format!("{name}Patch");
            if !components.schemas.contains_key(&patch_name) {
                let Some(utoipa::openapi::RefOr::T(utoipa::openapi::Schema::Object(object))) =
                    components.schemas.get(&name)
                else {
                    continue;
                };
                let mut relaxed = object.clone();
                relaxed.required.clear();
                relaxed.description = Some(format!(
                    "A partial {name}: only the properties present are merged. `id` is ignored."
                ));
                components.schemas.insert(
                    patch_name.clone(),
                    utoipa::openapi::RefOr::T(utoipa::openapi::Schema::Object(relaxed)),
                );
            }
            derived.push((name, patch_name));
        }

        for item in openapi.paths.paths.values_mut() {
            let Some(operation) = item.patch.as_mut() else {
                continue;
            };
            let Some(name) = patch_body_schema_name(operation) else {
                continue;
            };
            let Some((_, patch_name)) = derived.iter().find(|(from, _)| *from == name) else {
                continue;
            };
            if let Some(body) = operation.request_body.as_mut() {
                for content in body.content.values_mut() {
                    content.schema =
                        Some(utoipa::openapi::RefOr::Ref(Ref::from_schema_name(patch_name)));
                }
            }
        }
    }
}

/// The component name a `PATCH` operation's JSON body refers to, if it refers
/// to one by reference at all.
fn patch_body_schema_name(operation: &Operation) -> Option<String> {
    let body = operation.request_body.as_ref()?;
    let content = body.content.get("application/json")?;
    match content.schema.as_ref()? {
        utoipa::openapi::RefOr::Ref(reference) => {
            reference.ref_location.rsplit('/').next().map(str::to_string)
        }
        utoipa::openapi::RefOr::T(_) => None,
    }
}

/// Makes every error response in the document an RFC 9457 problem document.
///
/// Handlers already return [`crate::api_error::ApiError`] for every failure, so
/// this pass exists to stop the *document* from describing them any other way:
/// a `4xx` annotated as bare (no body at all, which is how every workspace 404
/// and 409 used to read) or as `text/plain` gets the same
/// `application/problem+json` body as everything else. A response with no
/// description also gets a usable one rather than the empty string utoipa emits.
pub struct ProblemJsonAddon;

impl Modify for ProblemJsonAddon {
    fn modify(&self, openapi: &mut OpenApi) {
        for operation in operations(openapi) {
            for (status, response) in operation.responses.responses.iter_mut() {
                // Only failures carry a problem document; a 2xx keeps whatever
                // body its handler actually returns.
                if !status.starts_with('4') && !status.starts_with('5') {
                    continue;
                }
                let utoipa::openapi::RefOr::T(response) = response else {
                    continue;
                };
                if response.description.is_empty() {
                    response.description = "Failed".to_string();
                }
                response.content.clear();
                response.content.insert(PROBLEM_JSON.to_string(), problem_content());
            }
        }
    }
}

/// Whether a security requirement actually demands a scheme, as opposed to
/// being the empty `{}` an operation declares to opt *out* of the document-wide
/// requirement. `SecurityRequirement` keeps its map private and exposes no
/// emptiness check, so this asks the serializer — the same view the emitted
/// document gets, which is the thing being reasoned about anyway.
fn is_real_requirement(requirement: &utoipa::openapi::security::SecurityRequirement) -> bool {
    serde_json::to_value(requirement)
        .ok()
        .and_then(|value| value.as_object().map(|map| !map.is_empty()))
        .unwrap_or(true)
}

/// The media type every error body is served as — the thing that tells a
/// generic client "this is a problem document, not the resource you asked for".
const PROBLEM_JSON: &str = "application/problem+json";

/// A `Content` entry pointing at the `ApiError` schema.
fn problem_content() -> utoipa::openapi::Content {
    utoipa::openapi::ContentBuilder::new()
        .schema(Some(Ref::from_schema_name("ApiError")))
        .build()
}

/// A complete problem-document response with the given description.
fn problem_response(description: &str) -> Response {
    ResponseBuilder::new()
        .description(description)
        .content(PROBLEM_JSON, problem_content())
        .build()
}

/// Every operation in the document, mutably — the traversal all three passes
/// share, so "for each endpoint" means the same set of endpoints in each.
fn operations(openapi: &mut OpenApi) -> impl Iterator<Item = &mut Operation> {
    openapi.paths.paths.values_mut().flat_map(|item| {
        [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ]
        .into_iter()
        .flatten()
    })
}
