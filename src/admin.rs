//! Transactional browser administration. No secret values enter audit facts.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sqlx::{Sqlite, Transaction};

use crate::{
    auth::{self, AuthState, CurrentUser, SecurityEvent},
    authorization::{Capabilities, Capability},
};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS admin_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_id TEXT NOT NULL,
    account_id TEXT,
    team_id TEXT,
    operation TEXT NOT NULL CHECK(operation IN ('create_account','enable','disable','set_role','reset_password','revoke_sessions','grant_source','revoke_source','revoke_token','create_team','rename_team','delete_team','add_member','remove_member','grant_team_source','revoke_team_source','require_mfa','release_mfa','reset_mfa')),
    subject TEXT NOT NULL,
    before_value TEXT NOT NULL,
    after_value TEXT NOT NULL,
    occurred_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_admin_events_time ON admin_events(occurred_at DESC,id DESC);
CREATE INDEX IF NOT EXISTS idx_security_events_time ON security_events(occurred_at DESC,id DESC);
CREATE TRIGGER IF NOT EXISTS admin_events_no_update BEFORE UPDATE ON admin_events BEGIN SELECT RAISE(ABORT,'administration events are append-only'); END;
"#;

pub(crate) async fn migrate(pool: &sqlx::SqlitePool) -> Result<()> {
    let schema: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='admin_events'",
    )
    .fetch_optional(pool)
    .await
    .context("inspect administrative audit vocabulary")?;
    if schema.is_none_or(|s| s.contains("'require_mfa'")) {
        return Ok(());
    }
    let mut tx = pool
        .begin()
        .await
        .context("begin MFA administrative audit migration")?;
    sqlx::raw_sql("DROP TRIGGER admin_events_no_update; DROP INDEX idx_admin_events_time; ALTER TABLE admin_events RENAME TO admin_events_legacy;").execute(&mut *tx).await.context("prepare MFA audit vocabulary")?;
    sqlx::raw_sql(SCHEMA)
        .execute(&mut *tx)
        .await
        .context("expand administrative audit vocabulary")?;
    sqlx::raw_sql("INSERT INTO admin_events SELECT * FROM admin_events_legacy; DROP TABLE admin_events_legacy;").execute(&mut *tx).await.context("preserve administrative audit facts")?;
    tx.commit().await.context("commit MFA audit migration")
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    CreateAccount,
    Enable,
    Disable,
    SetRole,
    ResetPassword,
    RevokeSessions,
    GrantSource,
    RevokeSource,
    RevokeToken,
    CreateTeam,
    RenameTeam,
    DeleteTeam,
    AddMember,
    RemoveMember,
    GrantTeamSource,
    RevokeTeamSource,
    RequireMfa,
    ReleaseMfa,
    ResetMfa,
}
impl Operation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CreateAccount => "create_account",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::SetRole => "set_role",
            Self::ResetPassword => "reset_password",
            Self::RevokeSessions => "revoke_sessions",
            Self::GrantSource => "grant_source",
            Self::RevokeSource => "revoke_source",
            Self::RevokeToken => "revoke_token",
            Self::CreateTeam => "create_team",
            Self::RenameTeam => "rename_team",
            Self::DeleteTeam => "delete_team",
            Self::AddMember => "add_member",
            Self::RemoveMember => "remove_member",
            Self::GrantTeamSource => "grant_team_source",
            Self::RevokeTeamSource => "revoke_team_source",
            Self::RequireMfa => "require_mfa",
            Self::ReleaseMfa => "release_mfa",
            Self::ResetMfa => "reset_mfa",
        }
    }
    fn team(self) -> bool {
        matches!(
            self,
            Self::RenameTeam
                | Self::DeleteTeam
                | Self::AddMember
                | Self::RemoveMember
                | Self::GrantTeamSource
                | Self::RevokeTeamSource
        )
    }
}

#[derive(Deserialize)]
pub(crate) struct Command {
    pub(crate) operation: Operation,
    pub(crate) target: String,
    pub(crate) confirmation: String,
    #[serde(default)]
    pub(crate) expected: String,
    #[serde(default)]
    pub(crate) value: String,
    #[serde(default)]
    pub(crate) new_password: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) role: String,
    pub(crate) enabled: bool,
    pub(crate) generation: i64,
    pub(crate) mfa_required: bool,
    pub(crate) mfa_enrolled: bool,
}
impl Account {
    pub(crate) fn version(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.role, self.enabled, self.generation, self.mfa_required, self.mfa_enrolled
        )
    }
}

pub(crate) async fn accounts(auth: &AuthState, offset: i64, limit: i64) -> Result<Vec<Account>> {
    sqlx::query_as("SELECT id,username,role,enabled,session_generation AS generation,COALESCE((SELECT required FROM account_mfa WHERE account_id=accounts.id),0) AS mfa_required,(EXISTS(SELECT 1 FROM account_mfa WHERE account_id=accounts.id AND secret IS NOT NULL) OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=accounts.id)) AS mfa_enrolled FROM accounts ORDER BY username COLLATE NOCASE,id LIMIT ? OFFSET ?")
        .bind(limit).bind(offset).fetch_all(&auth.pool).await.context("list administrative accounts")
}

pub(crate) async fn account(auth: &AuthState, id: &str) -> Result<Option<Account>> {
    sqlx::query_as(
        "SELECT id,username,role,enabled,session_generation AS generation,COALESCE((SELECT required FROM account_mfa WHERE account_id=accounts.id),0) AS mfa_required,(EXISTS(SELECT 1 FROM account_mfa WHERE account_id=accounts.id AND secret IS NOT NULL) OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=accounts.id)) AS mfa_enrolled FROM accounts WHERE id=?",
    )
    .bind(id)
    .fetch_optional(&auth.pool)
    .await
    .context("inspect administrative account")
}

#[derive(sqlx::FromRow)]
pub(crate) struct AuditEntry {
    pub(crate) occurred_at: String,
    pub(crate) actor: String,
    pub(crate) account: String,
    pub(crate) team: String,
    pub(crate) operation: String,
    pub(crate) subject: String,
    pub(crate) before_value: String,
    pub(crate) after_value: String,
}

pub(crate) async fn events(auth: &AuthState, offset: i64, limit: i64) -> Result<Vec<AuditEntry>> {
    sqlx::query_as(r#"
        SELECT strftime('%Y-%m-%dT%H:%M:%SZ',e.occurred_at,'unixepoch') AS occurred_at,
               actor,account,team,operation,subject,before_value,after_value
        FROM (
            SELECT id,occurred_at,actor_id AS actor,COALESCE(account_id,'') AS account,
                   COALESCE(team_id,'') AS team,operation,subject,before_value,after_value,'admin' AS origin
            FROM admin_events WHERE occurred_at>=unixepoch()-?
            UNION ALL
            SELECT id,occurred_at,'' AS actor,COALESCE(account_id,'') AS account,
                   COALESCE(team_id,'') AS team,event_type AS operation,'' AS subject,
                   '' AS before_value,'' AS after_value,'security' AS origin
            FROM security_events WHERE occurred_at>=unixepoch()-?
        ) AS e ORDER BY e.occurred_at DESC,e.id DESC,e.origin LIMIT ? OFFSET ?
        "#)
        .bind(auth::SECURITY_RETENTION_DAYS*86_400).bind(auth::SECURITY_RETENTION_DAYS*86_400)
        .bind(limit).bind(offset).fetch_all(&auth.pool).await.context("review bounded administrative and security history")
}

/// Recheck the live actor and session after acquiring SQLite's writer lock.
/// All policy reads, target checks, mutations, and audit writes share this lock.
pub(crate) async fn execute(
    auth: &AuthState,
    actor: &CurrentUser,
    inventory: &str,
    command: Command,
) -> Result<()> {
    ensure!(
        !auth.is_local_mode(),
        "administration requires an application identity"
    );
    ensure!(
        actor.access.permits(Capability::AccountsAdminister),
        "administrator capability required"
    );
    let password_hash = if matches!(
        command.operation,
        Operation::CreateAccount | Operation::ResetPassword
    ) {
        auth::validate_password(&command.new_password)?;
        let password = command.new_password.clone();
        Some(
            tokio::task::spawn_blocking(move || auth::hash_password(&password))
                .await
                .context("join administrative password hashing")??,
        )
    } else {
        None
    };
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin browser administration")?;
    let role: Option<String> = sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=? AND EXISTS(SELECT 1 FROM account_sessions s WHERE s.id=? AND s.account_id=accounts.id AND s.session_generation=accounts.session_generation AND s.revoked_at IS NULL AND s.absolute_expires_at>unixepoch() AND s.last_seen_at>=unixepoch()-1800) RETURNING role")
        .bind(&actor.id).bind(actor.credential_generation).bind(inventory).fetch_optional(&mut *tx).await.context("lock and recheck administrative identity")?;
    let role = auth::parse_role(&role.context("administrator session no longer valid")?)?;
    ensure!(
        Capabilities::for_role(role).permits(Capability::AccountsAdminister),
        "administrator capability no longer available"
    );
    let mfa_required:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_mfa WHERE account_id=? AND required=1 AND secret IS NULL AND NOT EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=account_mfa.account_id))").bind(&actor.id).fetch_one(&mut *tx).await.context("recheck administrator MFA enrollment gate")?;
    ensure!(
        !mfa_required,
        "administrator must complete required MFA enrollment"
    );
    if matches!(command.operation, Operation::RequireMfa) {
        crate::mfa::prepare_key(auth, &mut tx).await?;
    }

    let mut facts = Facts {
        account_id: None,
        team_id: None,
        subject: String::new(),
        before: String::new(),
        after: String::new(),
    };
    let event = if matches!(command.operation, Operation::CreateAccount) {
        auth::validate_username(&command.target)?;
        ensure!(
            command.confirmation == command.target,
            "target confirmation does not match"
        );
        let role = auth::parse_role(&command.value)?;
        let id = auth::random_token();
        sqlx::query("INSERT INTO accounts(id,username,password_hash,role) VALUES(?,?,?,?)")
            .bind(&id)
            .bind(&command.target)
            .bind(password_hash)
            .bind(role.as_str())
            .execute(&mut *tx)
            .await
            .context("create administrative account")?;
        facts.account_id = Some(id);
        facts.before = "absent".into();
        facts.after = format!("enabled:{}", role.as_str());
        SecurityEvent::AccountCreated
    } else if matches!(command.operation, Operation::CreateTeam) {
        let name = crate::access::name(&command.target)?;
        ensure!(
            command.confirmation == name,
            "target confirmation does not match"
        );
        let id = format!("team_{}", auth::random_token());
        sqlx::query("INSERT INTO teams(id,name) VALUES(?,?)")
            .bind(&id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .context("create administrative team")?;
        facts.team_id = Some(id);
        facts.before = "absent".into();
        facts.after = name.into();
        SecurityEvent::TeamCreated
    } else if command.operation.team() {
        team_change(&mut tx, &command, &mut facts).await?
    } else {
        let target: Account=sqlx::query_as("SELECT id,username,role,enabled,session_generation AS generation,COALESCE((SELECT required FROM account_mfa WHERE account_id=accounts.id),0) AS mfa_required,(EXISTS(SELECT 1 FROM account_mfa WHERE account_id=accounts.id AND secret IS NOT NULL) OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=accounts.id)) AS mfa_enrolled FROM accounts WHERE id=?")
            .bind(&command.target).fetch_optional(&mut *tx).await.context("resolve administrative target")?.context("target account not found")?;
        ensure!(
            command.confirmation == target.username,
            "target confirmation does not match"
        );
        ensure!(
            command.expected == target.version(),
            "account changed; reload and confirm the current state"
        );
        facts.account_id = Some(target.id.clone());
        account_change(&mut tx, &target, &command, password_hash, &mut facts).await?
    };
    // Keep the existing affected-account security history in the same transaction.
    if let Some(team) = &facts.team_id {
        crate::access::event(&mut tx, facts.account_id.as_deref(), team, event).await?;
    } else {
        auth::insert_security_event(&mut tx, facts.account_id.as_deref(), event).await?;
    }
    sqlx::query("DELETE FROM admin_events WHERE occurred_at<unixepoch()-?")
        .bind(auth::SECURITY_RETENTION_DAYS * 86_400)
        .execute(&mut *tx)
        .await
        .context("prune administrative history")?;
    sqlx::query("INSERT INTO admin_events(actor_id,account_id,team_id,operation,subject,before_value,after_value) VALUES(?,?,?,?,?,?,?)")
        .bind(&actor.id).bind(&facts.account_id).bind(&facts.team_id).bind(command.operation.as_str()).bind(&facts.subject).bind(&facts.before).bind(&facts.after)
        .execute(&mut *tx).await.context("append administrative audit facts")?;
    tx.commit().await.context("commit browser administration")
}

struct Facts {
    account_id: Option<String>,
    team_id: Option<String>,
    subject: String,
    before: String,
    after: String,
}

async fn account_change(
    tx: &mut Transaction<'_, Sqlite>,
    target: &Account,
    command: &Command,
    hash: Option<String>,
    facts: &mut Facts,
) -> Result<SecurityEvent> {
    let id = &target.id;
    Ok(match command.operation {
        Operation::RequireMfa | Operation::ReleaseMfa | Operation::ResetMfa => {
            facts.before = format!(
                "required:{},enrolled:{}",
                target.mfa_required, target.mfa_enrolled
            );
            if matches!(command.operation, Operation::ResetMfa) {
                crate::mfa::reset_in_transaction(tx, id).await?;
                facts.after = format!("required:{},enrolled:false", target.mfa_required);
            } else {
                let required = matches!(command.operation, Operation::RequireMfa);
                sqlx::query("INSERT INTO account_mfa(account_id,required) VALUES(?,?) ON CONFLICT(account_id) DO UPDATE SET required=excluded.required")
                    .bind(id).bind(required).execute(&mut **tx).await.context("set mandatory MFA enrollment policy")?;
                facts.after = format!("required:{required},enrolled:{}", target.mfa_enrolled);
            }
            crate::mfa::invalidate(tx, id).await?;
            match command.operation {
                Operation::RequireMfa => SecurityEvent::MfaRequired,
                Operation::ReleaseMfa => SecurityEvent::MfaReleased,
                _ => SecurityEvent::MfaReset,
            }
        }
        Operation::Enable | Operation::Disable => {
            let enabled = matches!(command.operation, Operation::Enable);
            facts.before = target.enabled.to_string();
            facts.after = enabled.to_string();
            sqlx::query(
                "UPDATE accounts SET enabled=?,session_generation=session_generation+? WHERE id=?",
            )
            .bind(enabled)
            .bind(i64::from(!enabled))
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("change account enabled state")?;
            if !enabled {
                auth::revoke_inventory_in_transaction(tx, id).await?;
            }
            if enabled {
                SecurityEvent::AccountEnabled
            } else {
                SecurityEvent::AccountDisabled
            }
        }
        Operation::SetRole => {
            let role = auth::parse_role(&command.value)?;
            facts.before = target.role.clone();
            facts.after = role.as_str().into();
            sqlx::query("UPDATE accounts SET role=? WHERE id=?")
                .bind(role.as_str())
                .bind(id)
                .execute(&mut **tx)
                .await
                .context("change administrative role")?;
            SecurityEvent::RoleChanged
        }
        Operation::ResetPassword | Operation::RevokeSessions => {
            facts.before = target.generation.to_string();
            facts.after = target
                .generation
                .checked_add(1)
                .context("credential generation exhausted")?
                .to_string();
            if matches!(command.operation, Operation::ResetPassword) {
                sqlx::query("UPDATE accounts SET password_hash=?,password_changed_at=CURRENT_TIMESTAMP,session_generation=session_generation+1 WHERE id=?")
                    .bind(hash).bind(id).execute(&mut **tx).await.context("reset administrative credential")?;
            } else {
                sqlx::query(
                    "UPDATE accounts SET session_generation=session_generation+1 WHERE id=?",
                )
                .bind(id)
                .execute(&mut **tx)
                .await
                .context("revoke administrative target sessions")?;
            }
            auth::revoke_inventory_in_transaction(tx, id).await?;
            if matches!(command.operation, Operation::ResetPassword) {
                SecurityEvent::PasswordReset
            } else {
                SecurityEvent::SessionsRevoked
            }
        }
        Operation::GrantSource | Operation::RevokeSource => {
            auth::validate_source_key(&command.value)?;
            facts.subject = command.value.clone();
            let present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_source_grants WHERE account_id=? AND source_key=?)").bind(id).bind(&command.value).fetch_one(&mut **tx).await?;
            let add = matches!(command.operation, Operation::GrantSource);
            facts.before = present.to_string();
            facts.after = add.to_string();
            let sql = if add {
                "INSERT OR IGNORE INTO account_source_grants(account_id,source_key) VALUES(?,?)"
            } else {
                "DELETE FROM account_source_grants WHERE account_id=? AND source_key=?"
            };
            sqlx::query(sql)
                .bind(id)
                .bind(&command.value)
                .execute(&mut **tx)
                .await
                .context("change administrative source grant")?;
            if add {
                SecurityEvent::SourceGranted
            } else {
                SecurityEvent::SourceRevoked
            }
        }
        Operation::RevokeToken => {
            let before:bool=sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM personal_access_tokens WHERE id=? AND account_id=?")
                .bind(&command.value).bind(id).fetch_optional(&mut **tx).await?.context("target token not found")?;
            facts.subject = command.value.clone();
            facts.before = if before { "revoked" } else { "unrevoked" }.into();
            facts.after = "revoked".into();
            sqlx::query("UPDATE personal_access_tokens SET revoked_at=COALESCE(revoked_at,unixepoch()) WHERE id=? AND account_id=?")
                .bind(&command.value).bind(id).execute(&mut **tx).await.context("administratively revoke token")?;
            SecurityEvent::TokenRevoked
        }
        _ => anyhow::bail!("invalid account operation"),
    })
}

async fn team_change(
    tx: &mut Transaction<'_, Sqlite>,
    command: &Command,
    facts: &mut Facts,
) -> Result<SecurityEvent> {
    let name: String = sqlx::query_scalar("SELECT name FROM teams WHERE id=?")
        .bind(&command.target)
        .fetch_optional(&mut **tx)
        .await?
        .context("target team not found")?;
    ensure!(
        command.confirmation == name,
        "target confirmation does not match"
    );
    facts.team_id = Some(command.target.clone());
    Ok(match command.operation {
        Operation::RenameTeam => {
            let replacement = crate::access::name(&command.value)?;
            facts.before = name;
            facts.after = replacement.into();
            sqlx::query("UPDATE teams SET name=? WHERE id=?")
                .bind(replacement)
                .bind(&command.target)
                .execute(&mut **tx)
                .await
                .context("rename administrative team")?;
            SecurityEvent::TeamRenamed
        }
        Operation::DeleteTeam => {
            facts.before = "present".into();
            facts.after = "absent".into();
            sqlx::query("DELETE FROM teams WHERE id=?")
                .bind(&command.target)
                .execute(&mut **tx)
                .await
                .context("delete administrative team")?;
            SecurityEvent::TeamDeleted
        }
        Operation::AddMember | Operation::RemoveMember => {
            let member: String =
                sqlx::query_scalar("SELECT id FROM accounts WHERE username=? COLLATE NOCASE")
                    .bind(&command.value)
                    .fetch_optional(&mut **tx)
                    .await?
                    .context("member account not found")?;
            let before: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id=? AND account_id=?)",
            )
            .bind(&command.target)
            .bind(&member)
            .fetch_one(&mut **tx)
            .await?;
            let add = matches!(command.operation, Operation::AddMember);
            facts.before = before.to_string();
            facts.after = add.to_string();
            let sql = if add {
                "INSERT OR IGNORE INTO team_members(team_id,account_id) VALUES(?,?)"
            } else {
                "DELETE FROM team_members WHERE team_id=? AND account_id=?"
            };
            sqlx::query(sql)
                .bind(&command.target)
                .bind(&member)
                .execute(&mut **tx)
                .await
                .context("change administrative team membership")?;
            facts.account_id = Some(member);
            if add {
                SecurityEvent::TeamMemberAdded
            } else {
                SecurityEvent::TeamMemberRemoved
            }
        }
        Operation::GrantTeamSource | Operation::RevokeTeamSource => {
            auth::validate_source_key(&command.value)?;
            facts.subject = command.value.clone();
            let before: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM team_source_grants WHERE team_id=? AND source_key=?)",
            )
            .bind(&command.target)
            .bind(&command.value)
            .fetch_one(&mut **tx)
            .await?;
            let add = matches!(command.operation, Operation::GrantTeamSource);
            facts.before = before.to_string();
            facts.after = add.to_string();
            let sql = if add {
                "INSERT OR IGNORE INTO team_source_grants(team_id,source_key) VALUES(?,?)"
            } else {
                "DELETE FROM team_source_grants WHERE team_id=? AND source_key=?"
            };
            sqlx::query(sql)
                .bind(&command.target)
                .bind(&command.value)
                .execute(&mut **tx)
                .await
                .context("change administrative team grant")?;
            if add {
                SecurityEvent::TeamSourceGranted
            } else {
                SecurityEvent::TeamSourceRevoked
            }
        }
        _ => anyhow::bail!("invalid team operation"),
    })
}
