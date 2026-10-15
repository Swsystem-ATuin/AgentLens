//! Shared policy vocabulary for browser identities and future scoped credentials.
use crate::auth::AccountRole;
use axum::http::Method;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Capability {
    ContentRead,
    ReportsRead,
    OperationsRead,
    AllSourcesRead,
    AccountSelfManage,
    AuditSelfRead,
    SourceRefresh,
    AnnotationsWrite,
    AccountsAdminister,
    AuditRead,
    TokensManage,
}

impl Capability {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ContentRead => "Read session content",
            Self::ReportsRead => "Read scoped reports",
            Self::OperationsRead => "Read installation health and metrics",
            Self::AllSourcesRead => "Read all sources",
            Self::AccountSelfManage => "Manage own account",
            Self::AuditSelfRead => "Read own security history",
            Self::TokensManage => "Manage own tokens",
            Self::SourceRefresh => "Refresh sources",
            Self::AnnotationsWrite => "Write annotations",
            Self::AccountsAdminister => "Administer accounts",
            Self::AuditRead => "Read installation audit history",
        }
    }
    pub(crate) const ALL: [Self; 11] = [
        Self::ContentRead,
        Self::ReportsRead,
        Self::OperationsRead,
        Self::AllSourcesRead,
        Self::AccountSelfManage,
        Self::AuditSelfRead,
        Self::SourceRefresh,
        Self::AnnotationsWrite,
        Self::AccountsAdminister,
        Self::AuditRead,
        Self::TokensManage,
    ];
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Capabilities(u16);

impl Capabilities {
    pub(crate) fn for_role(role: AccountRole) -> Self {
        // Reserved capabilities do not authorize unimplemented browser features.
        let allowed: &[Capability] = match role {
            AccountRole::Admin => &[
                Capability::ContentRead,
                Capability::ReportsRead,
                Capability::AccountSelfManage,
                Capability::AuditSelfRead,
                Capability::OperationsRead,
                Capability::AllSourcesRead,
                Capability::TokensManage,
            ],
            AccountRole::Viewer => &[
                Capability::ContentRead,
                Capability::ReportsRead,
                Capability::AccountSelfManage,
                Capability::AuditSelfRead,
                Capability::TokensManage,
            ],
            AccountRole::Auditor => &[
                Capability::ReportsRead,
                Capability::AccountSelfManage,
                Capability::AuditSelfRead,
                Capability::TokensManage,
            ],
            AccountRole::Operator => &[
                Capability::ContentRead,
                Capability::ReportsRead,
                Capability::OperationsRead,
                Capability::AccountSelfManage,
                Capability::AuditSelfRead,
                Capability::TokensManage,
            ],
        };
        Self(u16::MAX)
            .constrained(&Capability::ALL)
            .constrained(allowed)
    }

    pub(crate) fn permits(self, capability: Capability) -> bool {
        self.0 & (1 << capability as u8) != 0
    }

    /// Credential scopes can only reduce the capabilities of a live account.
    pub(crate) fn constrained(self, ceiling: &[Capability]) -> Self {
        let bits = ceiling
            .iter()
            .fold(0, |bits, capability| bits | (1 << *capability as u8));
        Self(self.0 & bits)
    }
}

/// Classify only implemented routes and their supported methods. Unknown routes
/// fail closed for every role, including administrators.
pub(crate) fn required_capability(method: &Method, path: &str) -> Option<Capability> {
    let read = *method == Method::GET || *method == Method::HEAD;
    if read {
        match path {
            "/account/tokens" => return Some(Capability::TokensManage),
            "/" => return Some(Capability::AccountSelfManage),
            "/sessions" | "/assets/live-reader.js" => return Some(Capability::ContentRead),
            "/usage" | "/diagnostics" => return Some(Capability::ReportsRead),
            "/health" | "/metrics" => return Some(Capability::OperationsRead),
            "/account"
            | "/account/access"
            | "/assets/preferences.js"
            | "/assets/preferences.css" => {
                return Some(Capability::AccountSelfManage);
            }
            "/account/sessions" => return Some(Capability::AuditSelfRead),
            _ => {}
        }
    } else if *method == Method::POST {
        match path {
            "/account/tokens" => return Some(Capability::TokensManage),
            "/account/password"
            | "/account/preferences"
            | "/account/profile"
            | "/logout"
            | "/account/sessions/revoke-others" => return Some(Capability::AccountSelfManage),
            _ => {}
        }
    } else {
        return None;
    }
    let parts: Vec<_> = path.strip_prefix('/')?.split('/').collect();
    match parts.as_slice() {
        ["account", "tokens", id] if !id.is_empty() && *method == Method::POST => {
            Some(Capability::TokensManage)
        }
        ["account", "sessions", id] if !id.is_empty() && *method == Method::POST => {
            Some(Capability::AccountSelfManage)
        }
        ["sessions", id] if read && !id.is_empty() => Some(Capability::ContentRead),
        ["sessions", id, "postmortem", "json" | "md"] if read && !id.is_empty() => {
            Some(Capability::ReportsRead)
        }
        ["sessions", id, "live-status" | "turns" | "generated-images"]
            if read && !id.is_empty() =>
        {
            Some(Capability::ContentRead)
        }
        [
            "sessions",
            id,
            "messages" | "tools" | "results" | "generated-images",
            item,
        ] if read && !id.is_empty() && !item.is_empty() => Some(Capability::ContentRead),
        _ => None,
    }
}

/// Only machine-readable read endpoints accept personal access tokens.
pub(crate) fn token_required_capability(method: &Method, path: &str) -> Option<Capability> {
    if *method != Method::GET && *method != Method::HEAD {
        return None;
    }
    if path == "/metrics" {
        return Some(Capability::OperationsRead);
    }
    let parts: Vec<_> = path.strip_prefix('/')?.split('/').collect();
    match parts.as_slice() {
        ["api", "sessions", id, "postmortem"] if !id.is_empty() => Some(Capability::ReportsRead),
        ["api", "sessions", id, "live-status"] if !id.is_empty() => Some(Capability::ContentRead),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_matrix_and_credential_ceilings_do_not_widen_access() {
        for role in [
            AccountRole::Admin,
            AccountRole::Viewer,
            AccountRole::Auditor,
            AccountRole::Operator,
        ] {
            let account = Capabilities::for_role(role);
            for capability in Capability::ALL {
                let expected = matches!(
                    capability,
                    Capability::ReportsRead
                        | Capability::AccountSelfManage
                        | Capability::AuditSelfRead
                        | Capability::TokensManage
                ) || (role != AccountRole::Auditor
                    && capability == Capability::ContentRead)
                    || (matches!(role, AccountRole::Admin | AccountRole::Operator)
                        && capability == Capability::OperationsRead)
                    || (role == AccountRole::Admin && capability == Capability::AllSourcesRead);
                assert_eq!(
                    account.permits(capability),
                    expected,
                    "{role:?}: {capability:?}"
                );
                let limited = account.constrained(&[capability]);
                for other in Capability::ALL {
                    assert_eq!(limited.permits(other), expected && other == capability);
                }
            }
        }
    }

    #[test]
    fn route_policy_rejects_unknown_routes_and_wrong_methods() {
        assert_eq!(
            required_capability(&Method::GET, "/sessions/synthetic/postmortem/json"),
            Some(Capability::ReportsRead)
        );
        assert_eq!(
            required_capability(&Method::HEAD, "/health"),
            Some(Capability::OperationsRead)
        );
        for (method, path) in [
            (Method::GET, "/future-data"),
            (Method::POST, "/sessions"),
            (Method::DELETE, "/account"),
            (Method::GET, "/account/sessions/revoke-others"),
            (Method::GET, "/sessions/synthetic/postmortem/raw"),
        ] {
            assert_eq!(required_capability(&method, path), None, "{method} {path}");
        }
    }
}
