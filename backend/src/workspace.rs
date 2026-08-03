//! The workspace: the backend's single source of truth for organizations,
//! departments, personas, groups, and settings.
//!
//! This is the data that used to live in the frontend's Zustand store. Making
//! the backend own it removes the "two copies drift apart" problem: the client
//! becomes a consumer that reads and mutates through the REST API.
//!
//! Everything is in memory and seeded on startup — this build never touches
//! disk. Persistence will slot in behind the same API later.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::models::{
    Department, Group, Memory, Organization, Persona, PersonaKind, PromptLabel, Settings, now_ms,
};

/// A serializable snapshot of the whole workspace — the on-disk persistence
/// format. Kept distinct from [`Workspace`] so the live type stays free to hold
/// non-persisted state later without changing the saved shape. camelCase to
/// match the wire types it is composed of.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub organizations: Vec<Organization>,
    pub departments: Vec<Department>,
    pub personas: Vec<Persona>,
    pub groups: Vec<Group>,
    pub settings: Settings,
    /// User-assigned names for persona identity hashes (hash → label). Absent in
    /// snapshots written before persona versioning existed.
    #[serde(default)]
    pub prompt_labels: HashMap<String, String>,
    /// Persona-scoped memories, each tagged with the identity hash that wrote it.
    /// Absent in snapshots written before memory existed.
    #[serde(default)]
    pub memories: Vec<Memory>,
}

/// The id of the default user identity, matching the frontend seed.
const DEFAULT_USER_PERSONA_ID: &str = "user-me";

/// The full editable workspace. All mutations go through the methods below so
/// invariants (cascade cleanup, "always one identity") hold in one place.
pub struct Workspace {
    pub organizations: Vec<Organization>,
    pub departments: Vec<Department>,
    pub personas: Vec<Persona>,
    pub groups: Vec<Group>,
    pub settings: Settings,
    /// User-assigned names for persona identity hashes (hash → label). Kept off
    /// [`Persona`] so naming a version never mutates the persona.
    pub prompt_labels: HashMap<String, String>,
    /// Persona-scoped memories, each stamped on write with the persona's identity
    /// hash. Keeping that stamp lets a later recall path scope reads to the
    /// persona's *current* hash, so a rewritten character never recalls an earlier
    /// version's memories — while nothing is silently deleted when a persona is
    /// edited.
    pub memories: Vec<Memory>,
}

/// One member of a group, as an agent sees it: a name, an optional blurb, and
/// whether this is the human "you". Used to give each agent a roster of who is
/// in the room (including the user).
#[derive(Clone, Debug)]
pub struct RosterMember {
    pub name: String,
    pub blurb: Option<String>,
    pub is_self: bool,
}

/// Why a persona create/update was refused. Names are globally unique (so the
/// lookup tool can address anyone by name) and there is exactly one user
/// identity, so both are enforced here rather than at the edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaError {
    /// No persona with the given id (update only).
    NotFound,
    /// The merged result isn't a persona — see [`PatchError::Invalid`].
    Invalid,
    /// Another persona already uses this name.
    NameTaken,
    /// Refused to create/produce a second user identity — there is only ever one.
    UserExists,
}

/// Why a partial update was refused, for the resources whose only two failure
/// modes these are.
///
/// The distinction matters on the wire: these used to collapse into a single
/// `None`, which the route layer could only report as 404 — so
/// `PATCH /groups/{id}` with `{"name": 123}` claimed the group didn't exist.
/// The same malformed patch against `/settings` reported 422, because that
/// handler happened to map `None` the other way. One failure, two answers,
/// neither reliable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchError {
    /// No resource with the given id.
    NotFound,
    /// The patch merged onto the resource, but the result no longer
    /// deserializes as one — a field given the wrong JSON type, or a required
    /// field explicitly set to `null`.
    Invalid,
}

/// A newly minted id for a created resource.
fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Whether a client-proposed id is one we're willing to store.
///
/// Ids become filenames — `messages/<group_id>.json`, `usage/<group_id>/…` —
/// and [`crate::persist::sanitize`] rewrites anything outside this set to `_`
/// before they get there. That stops a path traversal, but it is a *lossy*
/// mapping: a client that created groups `a/b` and `a_b` would get two distinct
/// groups in the workspace sharing a single message log, each silently reading
/// and overwriting the other's history. Rejecting the id up front makes
/// `sanitize` a second line of defence rather than a semantic transform, and
/// the two can no longer disagree about what an id means.
///
/// The length bound is for the same reason: a filename has one, and an id long
/// enough to exceed it would fail to persist at write time, far from the
/// request that caused it.
fn usable_id(proposed: &str) -> bool {
    !proposed.is_empty()
        && proposed.len() <= 64
        && proposed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Resolves the id for a resource being created. The client may supply its own
/// id in the POST body (so it can insert optimistically and stay in sync with
/// the server without a round-trip); we honour it when it's usable (see
/// [`usable_id`]) and not already taken, and otherwise mint a fresh one.
///
/// A rejected id is replaced rather than refused: the response carries the id
/// that was actually stored, so a client that reads it back stays in sync
/// either way, and there is no reason to fail a create over a detail the
/// server is willing to decide.
fn resolve_id<'a>(proposed: String, existing: impl Iterator<Item = &'a str>) -> String {
    if !usable_id(&proposed) {
        return new_id();
    }
    if existing.into_iter().any(|id| id == proposed) {
        return new_id();
    }
    proposed
}

/// Merges a partial JSON `patch` onto a serializable resource, mirroring the
/// client's `{ ...existing, ...patch }`: keys present in the patch overwrite,
/// absent keys are left untouched. Returns the updated, re-validated resource,
/// or `None` if the merge produced something that no longer fits the type.
fn apply_patch<T>(existing: &T, patch: Value) -> Option<T>
where
    T: serde::Serialize + for<'de> Deserialize<'de>,
{
    let mut base = serde_json::to_value(existing).ok()?;
    let (Value::Object(base_map), Value::Object(patch_map)) = (&mut base, patch) else {
        return None;
    };
    for (key, value) in patch_map {
        // Never let a patch rewrite the id.
        if key == "id" {
            continue;
        }
        base_map.insert(key, value);
    }
    serde_json::from_value(base).ok()
}

impl Workspace {
    pub fn seeded() -> Self {
        let mut workspace = Self {
            organizations: seed_organizations(),
            departments: seed_departments(),
            personas: seed_personas(),
            groups: seed_groups(),
            settings: seed_settings(),
            prompt_labels: HashMap::new(),
            memories: Vec::new(),
        };
        workspace.refresh_prompt_hashes();
        workspace
    }

    /// Rebuilds a workspace from a persisted snapshot (loaded from disk).
    /// Older snapshots may carry several user identities; the single-user
    /// invariant is re-established on load so downstream code can assume one.
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Self {
        let mut workspace = Self {
            organizations: snapshot.organizations,
            departments: snapshot.departments,
            personas: snapshot.personas,
            groups: snapshot.groups,
            settings: snapshot.settings,
            prompt_labels: snapshot.prompt_labels,
            memories: snapshot.memories,
        };
        workspace.enforce_single_user();
        // Recompute hashes from the loaded prompts: a snapshot may predate
        // versioning, or have been hand-edited, so the stored value isn't trusted.
        workspace.refresh_prompt_hashes();
        workspace
    }

    /// Recomputes every persona's identity hash from its current system prompt,
    /// so the stored value is authoritative regardless of what a snapshot carried.
    fn refresh_prompt_hashes(&mut self) {
        for persona in &mut self.personas {
            persona.refresh_prompt_hash();
        }
    }

    /// Collapses to exactly one user identity: keeps the first user persona,
    /// drops any others, and repoints group `self_persona_id`s (and membership)
    /// off the removed identities. Synthesizes the default "you" if none exists.
    fn enforce_single_user(&mut self) {
        let users: Vec<String> = self
            .personas
            .iter()
            .filter(|p| p.kind == PersonaKind::User)
            .map(|p| p.id.clone())
            .collect();
        let Some(keep) = users.first().cloned() else {
            // No user at all (corrupt data): restore the default identity.
            self.personas.insert(0, default_user_persona());
            for g in &mut self.groups {
                g.self_persona_id = DEFAULT_USER_PERSONA_ID.into();
            }
            return;
        };
        let dropped: HashSet<String> = users.into_iter().skip(1).collect();
        if dropped.is_empty() {
            return;
        }
        self.personas.retain(|p| !dropped.contains(&p.id));
        for g in &mut self.groups {
            if dropped.contains(&g.self_persona_id) {
                g.self_persona_id = keep.clone();
            }
            g.persona_ids.retain(|pid| !dropped.contains(pid));
        }
    }

    /// True when another persona (any except `except_id`) already uses `name`,
    /// compared case-insensitively and trimmed. The basis for global name
    /// uniqueness.
    fn name_taken(&self, name: &str, except_id: Option<&str>) -> bool {
        let name = name.trim();
        self.personas
            .iter()
            .any(|p| Some(p.id.as_str()) != except_id && p.name.trim().eq_ignore_ascii_case(name))
    }

    /// A serializable copy of the workspace, for writing to disk.
    pub fn to_snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            organizations: self.organizations.clone(),
            departments: self.departments.clone(),
            personas: self.personas.clone(),
            groups: self.groups.clone(),
            settings: self.settings.clone(),
            prompt_labels: self.prompt_labels.clone(),
            memories: self.memories.clone(),
        }
    }

    // --- Organizations ------------------------------------------------------

    pub fn create_organization(&mut self, mut org: Organization) -> Organization {
        org.id = resolve_id(org.id, self.organizations.iter().map(|o| o.id.as_str()));
        self.organizations.push(org.clone());
        org
    }

    pub fn update_organization(
        &mut self,
        id: &str,
        patch: Value,
    ) -> Result<Organization, PatchError> {
        let org = self.organizations.iter().find(|o| o.id == id).ok_or(PatchError::NotFound)?;
        let updated = apply_patch(org, patch).ok_or(PatchError::Invalid)?;
        let slot =
            self.organizations.iter_mut().find(|o| o.id == id).ok_or(PatchError::NotFound)?;
        *slot = updated.clone();
        Ok(updated)
    }

    /// Removes an organization and its departments, and clears the now-dangling
    /// org/department links on member personas (mirrors the frontend cascade).
    pub fn delete_organization(&mut self, id: &str) -> bool {
        if !self.organizations.iter().any(|o| o.id == id) {
            return false;
        }
        let removed_depts: HashSet<String> = self
            .departments
            .iter()
            .filter(|d| d.organization_id == id)
            .map(|d| d.id.clone())
            .collect();
        self.organizations.retain(|o| o.id != id);
        self.departments.retain(|d| d.organization_id != id);
        for p in &mut self.personas {
            let in_org = p.organization_id.as_deref() == Some(id);
            let in_dept = p
                .department_id
                .as_ref()
                .is_some_and(|d| removed_depts.contains(d));
            if in_org || in_dept {
                p.organization_id = None;
                p.department_id = None;
            }
        }
        true
    }

    // --- Departments --------------------------------------------------------

    pub fn create_department(&mut self, mut dept: Department) -> Department {
        dept.id = resolve_id(dept.id, self.departments.iter().map(|d| d.id.as_str()));
        self.departments.push(dept.clone());
        dept
    }

    pub fn update_department(&mut self, id: &str, patch: Value) -> Result<Department, PatchError> {
        let dept = self.departments.iter().find(|d| d.id == id).ok_or(PatchError::NotFound)?;
        let updated = apply_patch(dept, patch).ok_or(PatchError::Invalid)?;
        let slot = self.departments.iter_mut().find(|d| d.id == id).ok_or(PatchError::NotFound)?;
        *slot = updated.clone();
        Ok(updated)
    }

    pub fn delete_department(&mut self, id: &str) -> bool {
        if !self.departments.iter().any(|d| d.id == id) {
            return false;
        }
        self.departments.retain(|d| d.id != id);
        for p in &mut self.personas {
            if p.department_id.as_deref() == Some(id) {
                p.department_id = None;
            }
        }
        true
    }

    // --- Personas -----------------------------------------------------------

    /// Creates a persona. Refuses a second user identity ([`PersonaError::UserExists`])
    /// and a name already in use ([`PersonaError::NameTaken`]).
    pub fn create_persona(&mut self, mut persona: Persona) -> Result<Persona, PersonaError> {
        if persona.kind == PersonaKind::User
            && self.personas.iter().any(|p| p.kind == PersonaKind::User)
        {
            return Err(PersonaError::UserExists);
        }
        if self.name_taken(&persona.name, None) {
            return Err(PersonaError::NameTaken);
        }
        persona.id = resolve_id(persona.id, self.personas.iter().map(|p| p.id.as_str()));
        persona.refresh_prompt_hash();
        self.personas.push(persona.clone());
        Ok(persona)
    }

    /// Applies a partial update. Rejects an unknown id ([`PersonaError::NotFound`]),
    /// a name collision ([`PersonaError::NameTaken`]), and any change that would
    /// yield a second user identity ([`PersonaError::UserExists`]).
    pub fn update_persona(&mut self, id: &str, patch: Value) -> Result<Persona, PersonaError> {
        let persona = self.personas.iter().find(|p| p.id == id).ok_or(PersonaError::NotFound)?;
        let mut updated = apply_patch(persona, patch).ok_or(PersonaError::Invalid)?;
        // The prompt may have changed; the client can't set the hash — recompute.
        updated.refresh_prompt_hash();
        if self.name_taken(&updated.name, Some(id)) {
            return Err(PersonaError::NameTaken);
        }
        if updated.kind == PersonaKind::User
            && self.personas.iter().any(|p| p.kind == PersonaKind::User && p.id != id)
        {
            return Err(PersonaError::UserExists);
        }
        let slot = self.personas.iter_mut().find(|p| p.id == id).ok_or(PersonaError::NotFound)?;
        *slot = updated.clone();
        Ok(updated)
    }

    /// Deletes a persona. Keeps at least one user identity around (groups always
    /// need a "you"), drops the id from group membership, and reassigns any
    /// group whose `selfPersonaId` was the deleted identity.
    pub fn delete_persona(&mut self, id: &str) -> bool {
        let Some(target) = self.personas.iter().find(|p| p.id == id) else {
            return false;
        };
        if target.kind == PersonaKind::User
            && self
                .personas
                .iter()
                .filter(|p| p.kind == PersonaKind::User)
                .count()
                <= 1
        {
            return false;
        }
        let fallback_self = self
            .personas
            .iter()
            .find(|p| p.kind == PersonaKind::User && p.id != id)
            .map(|p| p.id.clone());

        self.personas.retain(|p| p.id != id);
        // A deleted persona's memories have nothing left to belong to.
        self.memories.retain(|m| m.persona_id != id);
        for g in &mut self.groups {
            g.persona_ids.retain(|pid| pid != id);
            if g.self_persona_id == id && let Some(fallback) = &fallback_self {
                g.self_persona_id = fallback.clone();
            }
        }
        true
    }

    // --- Prompt identity labels ---------------------------------------------

    /// Every named identity hash, sorted by hash for a stable listing.
    pub fn prompt_labels(&self) -> Vec<PromptLabel> {
        let mut labels: Vec<PromptLabel> = self
            .prompt_labels
            .iter()
            .map(|(hash, label)| PromptLabel { hash: hash.clone(), label: label.clone() })
            .collect();
        labels.sort_by(|a, b| a.hash.cmp(&b.hash));
        labels
    }

    /// Names an identity hash, or clears its name when `label` is blank (trimmed).
    /// Returns the resulting label — an empty string when cleared.
    pub fn set_prompt_label(&mut self, hash: &str, label: &str) -> PromptLabel {
        let label = label.trim();
        if label.is_empty() {
            self.prompt_labels.remove(hash);
        } else {
            self.prompt_labels.insert(hash.to_string(), label.to_string());
        }
        PromptLabel { hash: hash.to_string(), label: label.to_string() }
    }

    // --- Persona memory -----------------------------------------------------

    /// A persona's current identity hash, if it has a prompt. The scope key for
    /// its memories.
    fn persona_hash(&self, persona_id: &str) -> Option<String> {
        self.personas.iter().find(|p| p.id == persona_id).and_then(|p| p.prompt_hash.clone())
    }

    /// Every memory a persona has accumulated, across all of its identity
    /// versions, newest first. The memory-management UI reads this and groups the
    /// result by `prompt_hash`/label.
    pub fn persona_memories(&self, persona_id: &str) -> Vec<Memory> {
        let mut mems: Vec<Memory> =
            self.memories.iter().filter(|m| m.persona_id == persona_id).cloned().collect();
        mems.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        mems
    }

    /// The memories a persona may *recall* right now: only those written under its
    /// current identity hash, newest first. This is the in-character subset of
    /// [`Self::persona_memories`] — memories an earlier version wrote are retained
    /// and still listed by the management UI, but held out of recall so a rewritten
    /// character doesn't answer from a former self's notes. A persona with no prompt
    /// (no hash to scope to) can recall nothing.
    pub fn recallable_memories(&self, persona_id: &str) -> Vec<Memory> {
        let Some(hash) = self.persona_hash(persona_id) else {
            return Vec::new();
        };
        let mut mems: Vec<Memory> = self
            .memories
            .iter()
            .filter(|m| m.persona_id == persona_id && m.prompt_hash == hash)
            .cloned()
            .collect();
        mems.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        mems
    }

    /// Records a memory for a persona, stamped with the persona's current identity
    /// hash so a later recall can keep it in-character. Returns `None` when the
    /// persona is unknown, has no prompt (no hash to scope to), or the content is
    /// blank.
    pub fn add_memory(&mut self, persona_id: &str, content: &str) -> Option<Memory> {
        let content = content.trim();
        if content.is_empty() {
            return None;
        }
        let prompt_hash = self.persona_hash(persona_id)?;
        let memory = Memory {
            id: new_id(),
            persona_id: persona_id.to_string(),
            prompt_hash,
            content: content.to_string(),
            created_at: now_ms(),
        };
        self.memories.push(memory.clone());
        Some(memory)
    }

    /// Deletes one of a persona's memories by id. Scoped to the persona so a
    /// memory id belonging to someone else can't be removed through its endpoint.
    /// Returns whether a memory was removed.
    pub fn delete_memory(&mut self, persona_id: &str, memory_id: &str) -> bool {
        let before = self.memories.len();
        self.memories.retain(|m| !(m.id == memory_id && m.persona_id == persona_id));
        self.memories.len() != before
    }

    // --- Groups -------------------------------------------------------------

    pub fn create_group(&mut self, mut group: Group) -> Group {
        group.id = resolve_id(group.id, self.groups.iter().map(|g| g.id.as_str()));
        self.groups.push(group.clone());
        group
    }

    pub fn update_group(&mut self, id: &str, patch: Value) -> Result<Group, PatchError> {
        let group = self.groups.iter().find(|g| g.id == id).ok_or(PatchError::NotFound)?;
        let updated = apply_patch(group, patch).ok_or(PatchError::Invalid)?;
        let slot = self.groups.iter_mut().find(|g| g.id == id).ok_or(PatchError::NotFound)?;
        *slot = updated.clone();
        Ok(updated)
    }

    pub fn delete_group(&mut self, id: &str) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        self.groups.len() != before
    }

    // --- Settings -----------------------------------------------------------

    pub fn update_settings(&mut self, patch: Value) -> Result<Settings, PatchError> {
        // There is exactly one settings record, so `NotFound` is unreachable
        // here — the only way this fails is a patch that doesn't fit.
        let updated = apply_patch(&self.settings, patch).ok_or(PatchError::Invalid)?;
        self.settings = updated.clone();
        Ok(updated)
    }

    // --- Turn helpers -------------------------------------------------------

    /// The people in a group — the user identity ("you") first, then everyone
    /// listed — with the name and blurb an agent needs to know who it's talking
    /// to. Returns `None` for an unknown group.
    pub fn group_roster(&self, group_id: &str) -> Option<Vec<RosterMember>> {
        let group = self.groups.iter().find(|g| g.id == group_id)?;
        let mut members = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();

        // The user identity leads, flagged so the prompt can mark it "(you)".
        if let Some(me) = self.personas.iter().find(|p| p.id == group.self_persona_id) {
            seen.insert(me.id.as_str());
            members.push(RosterMember {
                name: me.name.clone(),
                blurb: me.blurb.clone(),
                is_self: true,
            });
        }
        // Then the listed members, in order, skipping the self if it recurs.
        for pid in &group.persona_ids {
            if !seen.insert(pid.as_str()) {
                continue;
            }
            if let Some(p) = self.personas.iter().find(|p| &p.id == pid) {
                members.push(RosterMember {
                    name: p.name.clone(),
                    blurb: p.blurb.clone(),
                    is_self: false,
                });
            }
        }
        Some(members)
    }

    /// The user identity that authors outgoing messages in a group, plus the AI
    /// personas that read and may reply. Returns `None` for an unknown group.
    pub fn turn_members(&self, group_id: &str) -> Option<(String, Vec<String>)> {
        let group = self.groups.iter().find(|g| g.id == group_id)?;
        let ai: HashSet<&str> = self
            .personas
            .iter()
            .filter(|p| p.kind == PersonaKind::Ai)
            .map(|p| p.id.as_str())
            .collect();
        let ai_members = group
            .persona_ids
            .iter()
            .filter(|pid| ai.contains(pid.as_str()))
            .cloned()
            .collect();
        Some((group.self_persona_id.clone(), ai_members))
    }

    /// A single persona by id, cloned for use outside the workspace lock.
    pub fn persona(&self, id: &str) -> Option<Persona> {
        self.personas.iter().find(|p| p.id == id).cloned()
    }

    /// The template variables in scope for a persona, resolved down the
    /// inheritance chain: organization → department → persona, each level
    /// overriding the last. This is what a model needs to fill its prompt, so it
    /// lives in the SSOT rather than being recomputed per client.
    pub fn resolve_variables(&self, persona: &Persona) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if let Some(dept_id) = &persona.department_id
            && let Some(dept) = self.departments.iter().find(|d| &d.id == dept_id)
        {
            if let Some(org) = self.organizations.iter().find(|o| o.id == dept.organization_id)
                && let Some(v) = &org.variables
            {
                vars.extend(v.clone());
            }
            if let Some(v) = &dept.variables {
                vars.extend(v.clone());
            }
        } else if let Some(org_id) = &persona.organization_id
            && let Some(org) = self.organizations.iter().find(|o| &o.id == org_id)
            && let Some(v) = &org.variables
        {
            vars.extend(v.clone());
        }
        if let Some(v) = &persona.variables {
            vars.extend(v.clone());
        }
        vars
    }
}

// --- Seed data (mirrors frontend/src/store/workspace.ts) --------------------

fn seed_organizations() -> Vec<Organization> {
    vec![Organization {
        id: "aurora".into(),
        name: "Aurora Academy".into(),
        color: Some("indigo".into()),
        blurb: Some("A school whose classes and clubs share one bright near-future setting.".into()),
        variables: Some([("world".into(), "a bright near-future Tokyo".into())].into()),
    }]
}

fn seed_departments() -> Vec<Department> {
    vec![
        Department {
            id: "class-2a".into(),
            organization_id: "aurora".into(),
            name: "Class 2-A".into(),
            color: Some("violet".into()),
            blurb: None,
            variables: Some([("setting".into(), "a lively second-year classroom".into())].into()),
        },
        Department {
            id: "broadcast".into(),
            organization_id: "aurora".into(),
            name: "Broadcast Club".into(),
            color: Some("cyan".into()),
            blurb: None,
            variables: Some([("setting".into(), "the after-school broadcast room".into())].into()),
        },
    ]
}

/// The single user identity, created fresh. There is exactly one; a new install
/// seeds it and a snapshot missing a user restores it. Its name defaults to a
/// plain given name (the user renames it on their profile page) rather than
/// "You" — the "you"/"你" wording is a UI affordance, not the stored name.
fn default_user_persona() -> Persona {
    Persona {
        id: DEFAULT_USER_PERSONA_ID.into(),
        name: "Alex".into(),
        kind: PersonaKind::User,
        color: "gray".into(),
        avatar_url: None,
        emoji: Some("🧑".into()),
        gradient: Some("linear-gradient(135deg, #4dabf7, #4263eb)".into()),
        blurb: Some("Your own voice.".into()),
        organization_id: None,
        department_id: None,
        system_prompt: None,
        variables: None,
        prompt_hash: None,
    }
}

fn seed_personas() -> Vec<Persona> {
    vec![
        default_user_persona(),
        Persona {
            id: "aria".into(),
            name: "Aria".into(),
            kind: PersonaKind::Ai,
            color: "violet".into(),
            avatar_url: None,
            emoji: Some("🌟".into()),
            gradient: Some("linear-gradient(135deg, #b197fc, #4dabf7)".into()),
            blurb: Some("Warm, curious host persona.".into()),
            organization_id: Some("aurora".into()),
            department_id: Some("class-2a".into()),
            system_prompt: Some(
                "You are {{persona_name}} in {{department_name}}, a warm and curious host in {{setting}}. Always reply in {{user_language}}.".into(),
            ),
            variables: None,
            prompt_hash: None,
        },
        Persona {
            id: "nox".into(),
            name: "Nox".into(),
            kind: PersonaKind::Ai,
            color: "cyan".into(),
            avatar_url: None,
            emoji: Some("🌙".into()),
            gradient: Some("linear-gradient(135deg, #3bc9db, #4263eb)".into()),
            blurb: Some("Dry, analytical strategist.".into()),
            organization_id: Some("aurora".into()),
            department_id: Some("broadcast".into()),
            system_prompt: Some(
                "You are {{persona_name}} of {{department_name}}, a dry, analytical strategist in {{setting}}. Always reply in {{user_language}}.".into(),
            ),
            variables: None,
            prompt_hash: None,
        },
        Persona {
            id: "sol".into(),
            name: "Sol".into(),
            kind: PersonaKind::Ai,
            color: "orange".into(),
            avatar_url: None,
            emoji: Some("☀️".into()),
            gradient: Some("linear-gradient(135deg, #ffd43b, #ff922b)".into()),
            blurb: Some("Upbeat, energetic cheerleader.".into()),
            organization_id: Some("aurora".into()),
            department_id: Some("class-2a".into()),
            system_prompt: Some(
                "You are {{persona_name}} in {{department_name}}, an upbeat, energetic cheerleader in {{setting}}. Always reply in {{user_language}}.".into(),
            ),
            variables: None,
            prompt_hash: None,
        },
    ]
}

fn seed_groups() -> Vec<Group> {
    vec![
        Group {
            id: "lounge".into(),
            name: "The Lounge".into(),
            persona_ids: vec!["aria".into(), "nox".into(), "sol".into()],
            self_persona_id: DEFAULT_USER_PERSONA_ID.into(),
        },
        Group {
            id: "lab".into(),
            name: "Persona Lab".into(),
            persona_ids: vec!["aria".into(), "nox".into()],
            self_persona_id: DEFAULT_USER_PERSONA_ID.into(),
        },
    ]
}

fn seed_settings() -> Settings {
    Settings {
        ui_language: "zh-Hant".into(),
        native_language: "繁體中文".into(),
        chat_font_size: 15,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(id: &str, name: &str) -> Persona {
        Persona {
            id: id.into(),
            name: name.into(),
            kind: PersonaKind::Ai,
            color: "violet".into(),
            avatar_url: None,
            emoji: None,
            gradient: None,
            blurb: None,
            organization_id: None,
            department_id: None,
            system_prompt: None,
            variables: None,
            prompt_hash: None,
        }
    }

    #[test]
    fn create_persona_rejects_duplicate_name_case_insensitively() {
        let mut ws = Workspace::seeded();
        // "Aria" is seeded; a differently-cased duplicate is refused.
        let err = ws.create_persona(ai("clone", "aria")).unwrap_err();
        assert_eq!(err, PersonaError::NameTaken);
        // A fresh name is accepted.
        assert!(ws.create_persona(ai("fresh", "Vega")).is_ok());
    }

    /// Two ids that differ only outside `[A-Za-z0-9_-]` used to collapse onto
    /// one filename once persisted, so two real groups shared a message log.
    /// A client-proposed id that can't survive that mapping is replaced.
    #[test]
    fn a_client_proposed_id_that_would_collide_on_disk_is_replaced() {
        let mut ws = Workspace::seeded();

        let slashed = ws.create_group(Group {
            id: "a/b".into(),
            name: "Slashed".into(),
            persona_ids: vec![],
            self_persona_id: "me".into(),
        });
        let underscored = ws.create_group(Group {
            id: "a_b".into(),
            name: "Underscored".into(),
            persona_ids: vec![],
            self_persona_id: "me".into(),
        });

        assert_ne!(slashed.id, "a/b", "an id with a path separator is not stored verbatim");
        assert_eq!(underscored.id, "a_b", "an already-safe id is still honoured");
        assert_ne!(
            crate::persist::sanitize(&slashed.id),
            crate::persist::sanitize(&underscored.id),
            "the two groups must not share a persistence filename"
        );

        // Over-long ids go the same way — a filename has a length limit, and
        // failing at write time would be far from the request that caused it.
        let long = ws.create_group(Group {
            id: "x".repeat(300),
            name: "Long".into(),
            persona_ids: vec![],
            self_persona_id: "me".into(),
        });
        assert!(long.id.len() <= 64);
    }

    #[test]
    fn create_persona_rejects_second_user() {
        let mut ws = Workspace::seeded();
        let mut second = ai("me2", "Another Me");
        second.kind = PersonaKind::User;
        assert_eq!(ws.create_persona(second).unwrap_err(), PersonaError::UserExists);
    }

    #[test]
    fn update_persona_rejects_rename_onto_existing_name() {
        let mut ws = Workspace::seeded();
        // Rename Nox → Aria collides with the seeded Aria.
        let patch = serde_json::json!({ "name": "Aria" });
        assert_eq!(ws.update_persona("nox", patch).unwrap_err(), PersonaError::NameTaken);
        // Renaming to a free name works, and keeping your own name is fine.
        assert!(ws.update_persona("nox", serde_json::json!({ "name": "Nyx" })).is_ok());
        assert!(ws.update_persona("nyx_missing", serde_json::json!({ "name": "X" })).is_err());
    }

    #[test]
    fn snapshot_collapses_extra_user_identities() {
        // A legacy snapshot with two user identities, one used as a group's self.
        let mut extra = ai("alter-ego", "Masked");
        extra.kind = PersonaKind::User;
        let snapshot = WorkspaceSnapshot {
            organizations: vec![],
            departments: vec![],
            personas: vec![default_user_persona(), extra, ai("aria", "Aria")],
            groups: vec![Group {
                id: "g".into(),
                name: "G".into(),
                persona_ids: vec!["alter-ego".into(), "aria".into()],
                self_persona_id: "alter-ego".into(),
            }],
            settings: seed_settings(),
            prompt_labels: HashMap::new(),
            memories: Vec::new(),
        };
        let ws = Workspace::from_snapshot(snapshot);

        // Exactly one user identity survives, and it is the default "you".
        let users: Vec<&Persona> =
            ws.personas.iter().filter(|p| p.kind == PersonaKind::User).collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].id, DEFAULT_USER_PERSONA_ID);
        // The group's self and membership are repointed off the dropped identity.
        let group = &ws.groups[0];
        assert_eq!(group.self_persona_id, DEFAULT_USER_PERSONA_ID);
        assert!(!group.persona_ids.iter().any(|id| id == "alter-ego"));
        assert!(group.persona_ids.iter().any(|id| id == "aria"));
    }

    /// A persona with a system prompt gets a content hash; one without stays
    /// hashless, and any hash a client tries to supply is overwritten.
    #[test]
    fn create_persona_computes_prompt_hash() {
        let mut ws = Workspace::seeded();

        let mut p = ai("scribe", "Scribe");
        p.system_prompt = Some("You are Scribe, a careful note-taker.".into());
        p.prompt_hash = Some("client-supplied-garbage".into());
        let created = ws.create_persona(p).unwrap();
        assert_eq!(
            created.prompt_hash,
            crate::models::prompt_hash(Some("You are Scribe, a careful note-taker."))
        );
        assert!(created.prompt_hash.is_some());

        // No prompt → no hash (the user identity, and this AI without one).
        assert!(ws.create_persona(ai("blank", "Blank")).unwrap().prompt_hash.is_none());
    }

    /// Editing the prompt changes the hash; pasting the exact earlier text back
    /// resolves to the same hash (content-addressing, not a counter).
    #[test]
    fn update_persona_hash_tracks_and_restores_prompt() {
        let mut ws = Workspace::seeded();
        let original = ws.persona("aria").unwrap().prompt_hash.unwrap();

        let edited = ws
            .update_persona("aria", serde_json::json!({ "systemPrompt": "A brand new Aria." }))
            .unwrap();
        let edited_hash = edited.prompt_hash.unwrap();
        assert_ne!(edited_hash, original);

        // Re-resolve the seeded prompt text and paste it back verbatim.
        let seeded_text = "You are {{persona_name}} in {{department_name}}, a warm and curious host in {{setting}}. Always reply in {{user_language}}.";
        let restored = ws
            .update_persona("aria", serde_json::json!({ "systemPrompt": seeded_text }))
            .unwrap();
        assert_eq!(restored.prompt_hash, Some(original));
    }

    #[test]
    fn set_prompt_label_names_and_clears() {
        let mut ws = Workspace::seeded();
        let hash = ws.persona("aria").unwrap().prompt_hash.unwrap();

        let named = ws.set_prompt_label(&hash, "  bar 版  ");
        assert_eq!(named.label, "bar 版"); // trimmed
        assert_eq!(ws.prompt_labels(), vec![PromptLabel { hash: hash.clone(), label: "bar 版".into() }]);

        // A blank label clears the entry.
        let cleared = ws.set_prompt_label(&hash, "   ");
        assert_eq!(cleared.label, "");
        assert!(ws.prompt_labels().is_empty());
    }

    /// A memory is tagged with the persona's current hash; blank content and
    /// prompt-less personas are refused (nothing to scope a memory to).
    #[test]
    fn add_memory_tags_current_hash_and_refuses_hashless() {
        let mut ws = Workspace::seeded();
        let hash = ws.persona("aria").unwrap().prompt_hash.unwrap();

        let mem = ws.add_memory("aria", "  the user prefers tea over coffee  ").unwrap();
        assert_eq!(mem.prompt_hash, hash);
        assert_eq!(mem.content, "the user prefers tea over coffee"); // trimmed
        assert_eq!(mem.persona_id, "aria");

        // Blank content and a prompt-less persona (the user identity) get nothing.
        assert!(ws.add_memory("aria", "   ").is_none());
        assert!(ws.add_memory(DEFAULT_USER_PERSONA_ID, "anything").is_none());
        assert!(ws.add_memory("ghost", "anything").is_none());
    }

    /// Each memory is stamped with the identity version live when it was written,
    /// so rewriting the persona partitions old from new by `prompt_hash` while the
    /// management listing still shows everything. This is what lets a later recall
    /// path (the memory tool) keep an old version's memories out of character
    /// without deleting them.
    #[test]
    fn memories_are_tagged_per_identity_version() {
        let mut ws = Workspace::seeded();
        let v1 = ws.persona("aria").unwrap().prompt_hash.unwrap();
        ws.add_memory("aria", "remembered under the original Aria").unwrap();

        // Redefine the character; a memory written now carries the new hash.
        ws.update_persona("aria", serde_json::json!({ "systemPrompt": "A brand new Aria." }))
            .unwrap();
        let v2 = ws.persona("aria").unwrap().prompt_hash.unwrap();
        assert_ne!(v1, v2);
        ws.add_memory("aria", "remembered under the new Aria").unwrap();

        // Both are retained and listed; each is scoped to the version that wrote it.
        let all = ws.persona_memories("aria");
        assert_eq!(all.len(), 2);
        let under_v1: Vec<&str> =
            all.iter().filter(|m| m.prompt_hash == v1).map(|m| m.content.as_str()).collect();
        let under_v2: Vec<&str> =
            all.iter().filter(|m| m.prompt_hash == v2).map(|m| m.content.as_str()).collect();
        assert_eq!(under_v1, ["remembered under the original Aria"]);
        assert_eq!(under_v2, ["remembered under the new Aria"]);
    }

    /// Recall is scoped to the *current* identity: a memory written before the
    /// persona was rewritten stays listed by `persona_memories` but drops out of
    /// `recallable_memories`, so the recall tool never feeds a former self's notes
    /// back into a redefined character.
    #[test]
    fn recall_is_limited_to_the_current_identity() {
        let mut ws = Workspace::seeded();
        ws.add_memory("aria", "written under the original Aria").unwrap();

        // Both the full listing and recall see the one memory while the identity
        // is unchanged.
        assert_eq!(ws.persona_memories("aria").len(), 1);
        assert_eq!(ws.recallable_memories("aria").len(), 1);

        // Redefine the character, then record a memory under the new identity.
        ws.update_persona("aria", serde_json::json!({ "systemPrompt": "A brand new Aria." }))
            .unwrap();
        let v2 = ws.persona("aria").unwrap().prompt_hash.unwrap();
        ws.add_memory("aria", "written under the new Aria").unwrap();

        // The management listing still shows both; recall shows only the current one.
        assert_eq!(ws.persona_memories("aria").len(), 2);
        let recallable = ws.recallable_memories("aria");
        assert_eq!(recallable.len(), 1);
        assert_eq!(recallable[0].content, "written under the new Aria");
        assert!(recallable.iter().all(|m| m.prompt_hash == v2));
    }

    /// A persona with no system prompt has no identity hash to scope memories to,
    /// so it can recall nothing even if rows somehow exist for it.
    #[test]
    fn recall_is_empty_without_a_prompt() {
        let ws = Workspace::seeded();
        // The seeded user identity ("me") carries no system prompt.
        assert!(ws.persona("user-me").and_then(|p| p.prompt_hash).is_none());
        assert!(ws.recallable_memories("user-me").is_empty());
    }

    /// Deletion is scoped to the owning persona, and dropping a persona takes its
    /// memories with it.
    #[test]
    fn delete_memory_is_scoped_and_cascades_with_persona() {
        let mut ws = Workspace::seeded();
        let aria_mem = ws.add_memory("aria", "aria's note").unwrap();
        ws.add_memory("nox", "nox's note").unwrap();

        // Nox's endpoint can't delete Aria's memory.
        assert!(!ws.delete_memory("nox", &aria_mem.id));
        assert_eq!(ws.persona_memories("aria").len(), 1);
        // The owner can.
        assert!(ws.delete_memory("aria", &aria_mem.id));
        assert!(ws.persona_memories("aria").is_empty());

        // Deleting Nox removes its remaining memory too.
        ws.delete_persona("nox");
        assert!(ws.persona_memories("nox").is_empty());
    }
}
