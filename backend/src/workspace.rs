//! The workspace: the backend's single source of truth for organizations,
//! departments, personas, groups, and settings.
//!
//! This is the data that used to live in the frontend's Zustand store. Making
//! the backend own it removes the "two copies drift apart" problem: the client
//! becomes a consumer that reads and mutates through the REST API.
//!
//! Everything is in memory and seeded on startup — this build never touches
//! disk. Persistence will slot in behind the same API later.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::models::{Department, Group, Organization, Persona, PersonaKind, Settings};

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

/// A newly minted id for a created resource.
fn new_id() -> String {
    Uuid::new_v4().to_string()
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

    // --- Organizations ------------------------------------------------------

    pub fn create_organization(&mut self, mut org: Organization) -> Organization {
        org.id = new_id();
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
        dept.id = new_id();
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

    pub fn create_persona(&mut self, mut persona: Persona) -> Persona {
        persona.id = new_id();
        self.personas.push(persona.clone());
        persona
    }

    pub fn update_persona(&mut self, id: &str, patch: Value) -> Option<Persona> {
        let persona = self.personas.iter().find(|p| p.id == id)?;
        let updated = apply_patch(persona, patch)?;
        let slot = self.personas.iter_mut().find(|p| p.id == id)?;
        *slot = updated.clone();
        Some(updated)
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
        group.id = new_id();
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

fn seed_personas() -> Vec<Persona> {
    vec![
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
        },
        Persona {
            id: "alter-ego".into(),
            name: "Masked".into(),
            kind: PersonaKind::User,
            color: "dark".into(),
            avatar_url: None,
            emoji: Some("🎭".into()),
            gradient: Some("linear-gradient(135deg, #495057, #212529)".into()),
            blurb: Some("An anonymous alter-ego.".into()),
            organization_id: None,
            department_id: None,
            system_prompt: None,
            variables: None,
        },
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
