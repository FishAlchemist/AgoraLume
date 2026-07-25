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

use crate::models::{Department, Group, Organization, Persona, PersonaKind, Settings};

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
    /// Another persona already uses this name.
    NameTaken,
    /// Refused to create/produce a second user identity — there is only ever one.
    UserExists,
}

/// A newly minted id for a created resource.
fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Resolves the id for a resource being created. The client may supply its own
/// id in the POST body (so it can insert optimistically and stay in sync with
/// the server without a round-trip); we honour it when it's non-empty and not
/// already taken, and otherwise mint a fresh one.
fn resolve_id<'a>(proposed: String, existing: impl Iterator<Item = &'a str>) -> String {
    if proposed.is_empty() {
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
        Self {
            organizations: seed_organizations(),
            departments: seed_departments(),
            personas: seed_personas(),
            groups: seed_groups(),
            settings: seed_settings(),
        }
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
        };
        workspace.enforce_single_user();
        workspace
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
        }
    }

    // --- Organizations ------------------------------------------------------

    pub fn create_organization(&mut self, mut org: Organization) -> Organization {
        org.id = resolve_id(org.id, self.organizations.iter().map(|o| o.id.as_str()));
        self.organizations.push(org.clone());
        org
    }

    pub fn update_organization(&mut self, id: &str, patch: Value) -> Option<Organization> {
        let org = self.organizations.iter().find(|o| o.id == id)?;
        let updated = apply_patch(org, patch)?;
        let slot = self.organizations.iter_mut().find(|o| o.id == id)?;
        *slot = updated.clone();
        Some(updated)
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

    pub fn update_department(&mut self, id: &str, patch: Value) -> Option<Department> {
        let dept = self.departments.iter().find(|d| d.id == id)?;
        let updated = apply_patch(dept, patch)?;
        let slot = self.departments.iter_mut().find(|d| d.id == id)?;
        *slot = updated.clone();
        Some(updated)
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
        self.personas.push(persona.clone());
        Ok(persona)
    }

    /// Applies a partial update. Rejects an unknown id ([`PersonaError::NotFound`]),
    /// a name collision ([`PersonaError::NameTaken`]), and any change that would
    /// yield a second user identity ([`PersonaError::UserExists`]).
    pub fn update_persona(&mut self, id: &str, patch: Value) -> Result<Persona, PersonaError> {
        let persona = self.personas.iter().find(|p| p.id == id).ok_or(PersonaError::NotFound)?;
        let updated = apply_patch(persona, patch).ok_or(PersonaError::NotFound)?;
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
        for g in &mut self.groups {
            g.persona_ids.retain(|pid| pid != id);
            if g.self_persona_id == id && let Some(fallback) = &fallback_self {
                g.self_persona_id = fallback.clone();
            }
        }
        true
    }

    // --- Groups -------------------------------------------------------------

    pub fn create_group(&mut self, mut group: Group) -> Group {
        group.id = resolve_id(group.id, self.groups.iter().map(|g| g.id.as_str()));
        self.groups.push(group.clone());
        group
    }

    pub fn update_group(&mut self, id: &str, patch: Value) -> Option<Group> {
        let group = self.groups.iter().find(|g| g.id == id)?;
        let updated = apply_patch(group, patch)?;
        let slot = self.groups.iter_mut().find(|g| g.id == id)?;
        *slot = updated.clone();
        Some(updated)
    }

    pub fn delete_group(&mut self, id: &str) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        self.groups.len() != before
    }

    // --- Settings -----------------------------------------------------------

    pub fn update_settings(&mut self, patch: Value) -> Option<Settings> {
        let updated = apply_patch(&self.settings, patch)?;
        self.settings = updated.clone();
        Some(updated)
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

/// The single user identity ("you"), created fresh. There is exactly one; a new
/// install seeds it and a snapshot missing a user restores it.
fn default_user_persona() -> Persona {
    Persona {
        id: DEFAULT_USER_PERSONA_ID.into(),
        name: "You".into(),
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
}
