use serde::{Deserialize, Serialize};

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_USER: &str = "user";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Conversations,
    Messages,
    Agents,
    McpServers,
    Providers,
    Files,
    Chat,
    Sandbox,
    Computers,
    Models,
    AdminUsers,
    AdminAudit,
    AdminComputers,
    AdminProviders,
}

impl Scope {
    pub const ALL: [Scope; 14] = [
        Scope::Conversations,
        Scope::Messages,
        Scope::Agents,
        Scope::McpServers,
        Scope::Providers,
        Scope::Files,
        Scope::Chat,
        Scope::Sandbox,
        Scope::Computers,
        Scope::Models,
        Scope::AdminUsers,
        Scope::AdminAudit,
        Scope::AdminComputers,
        Scope::AdminProviders,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    View,
    Use,
    Manage,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::View, Action::Use, Action::Manage];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permission {
    pub scope: Scope,
    pub action: Action,
}

pub fn is_known_role(role: &str) -> bool {
    role == ROLE_ADMIN || role == ROLE_USER
}

pub fn role_allows(role: &str, scope: Scope, action: Action) -> bool {
    match role {
        ROLE_ADMIN => true,
        ROLE_USER => match scope {
            Scope::AdminUsers
            | Scope::AdminAudit
            | Scope::AdminComputers
            | Scope::AdminProviders => false,
            Scope::Chat | Scope::Sandbox => action == Action::Use,
            Scope::Models => action == Action::View,
            _ => true,
        },
        _ => false,
    }
}

pub fn permissions_for_role(role: &str) -> Vec<Permission> {
    Scope::ALL
        .iter()
        .flat_map(|&scope| {
            Action::ALL.iter().filter_map(move |&action| {
                role_allows(role, scope, action).then_some(Permission { scope, action })
            })
        })
        .collect()
}
