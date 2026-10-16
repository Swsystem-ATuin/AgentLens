# TOTP two-factor authentication

MFA is **opt in by default for every role**. Signed-in users enroll at `/account/mfa`; administrators can require enrollment, remove that requirement, or reset an account's MFA through `/admin/accounts/ACCOUNT_ID`. There is no role-wide automatic enforcement, remembered-device exception, public reset link, or notification delivery. Explicit loopback local mode remains development-only and cannot manage MFA.

[Opt-in passkeys](passkey-second-factors.md) are also available as password-first second factors once the public HTTPS URL is configured. Either enrolled method satisfies the same MFA requirement, and recovery codes are shared. Administrator/CLI reset and disabling all optional MFA remove both TOTP and passkeys; removing a requirement alone does not remove either factor.

## Enrollment and sign-in

Open Account → Manage two-factor authentication. Confirm your password and start setup. Import the displayed provisioning URI directly into a trusted authenticator, or enter its Base32 secret manually with SHA-1, six digits, and a 30-second period. No third-party QR service is used. Confirm a current authenticator code and your password to activate enrollment. Setup expires after ten minutes and belongs to the initiating browser inventory/session and credential generation; another session cannot finish it. Replacing pending setup invalidates its earlier secret. Unconfirmed expired ciphertext is removed at startup.

Activation shows **ten recovery codes once**, only in that POST response. Save them separately from the password and authenticator. Each 128-bit random recovery code replaces one authenticator code once. Only account-bound BLAKE3 digests are stored; neither the original codes nor provisioning secrets are stored in sessions or audit history. Later GET requests cannot retrieve them. Replacing recovery codes invalidates all old ones and shows the replacement set once.

Every browser sign-in requires username, password, and an authenticator/recovery code once enrolled. The same code field is shown for all accounts and failures use a generic username/password/code error. Successful TOTP verification atomically advances the last accepted time step, rejecting concurrent reuse and older steps. A one-step clock drift either way is accepted; keep server and authenticator clocks synchronized. A code used for enrollment or sign-in cannot immediately be reused for another action: wait for the next code or use a recovery code. There is no remembered-browser bypass.

MFA-protected administrators also supply a fresh factor for every administrative mutation. Token creation, password changes, recovery-code replacement, and disabling MFA require current credentials and a factor. MFA settings mutations are CSRF-protected. Disabling optional MFA, activating enrollment, and replacing recovery codes revoke all other browser sessions and all existing tokens, rotate the current browser session, and retain its completed factor proof. Optional MFA can be disabled by the account; required MFA cannot.

## Administrator enforcement and recovery

The account detail page shows enrolled and required states. Existing per-action password/factor confirmation, exact-target confirmation, stale-state checks, current capability/session checks, and before/after audit facts also cover MFA controls:

- **Require MFA enrollment:** invalidates the target's sessions/tokens. Its next password sign-in can access only setup, required interface assets, and sign-out until enrollment completes. Session lists, transcripts, reports, administration, and token creation remain inaccessible. Encryption key storage must be usable before the requirement can commit.
- **Remove MFA requirement:** removes the policy, not the enrolled authenticator. An already enrolled account must still provide a factor unless it explicitly disables optional MFA. Existing credentials are revoked.
- **Reset MFA for recovery:** removes the authenticator, pending enrollment, and recovery digests and revokes credentials. The enrollment requirement is deliberately preserved; a required account must enroll again before accessing application data. An administrator never sees another user's secret or recovery codes.

Password reset alone does not remove MFA. For lost factors, an authorized administrator can reset MFA after independently verifying the user's identity. Local filesystem-authorized recovery remains available, even if the encryption key is missing or unreadable:

```bash
cargo run -- account reset-mfa --username analyst
```

Use `--state-database` and `--mfa-key-file` before `account` when overriding their defaults. If encryption keys are irrecoverably lost, reset **each** affected enrolled/pending account using this CLI boundary before removing an unusable key file and starting new enrollments. Never edit raw agent data or manually weaken the enrollment requirement. The last enabled administrator cannot be disabled or demoted; requiring or resetting its MFA leaves password-authenticated enrollment and CLI recovery available.

## Automation and throttling

Password-only metrics Basic authentication is denied for enrolled or required accounts. Use a least-privilege, expiring [personal access token](personal-access-tokens.md) instead. Tokens intentionally remain noninteractive credentials and do not require a TOTP on each request. Forcing/resetting/changing MFA invalidates existing tokens via credential generation; required-but-unenrolled accounts cannot mint or authenticate tokens. A user must finish enrollment before creating replacement tokens, with password and factor confirmation.

The shared 60-password-attempt/60-second process budget covers login and password-confirmed management. Factor verification additionally permits ten attempts per five-minute account window, persisted across restarts and updated transactionally after password confirmation. Recovery codes share this budget. Exhaustion denies factor verification until the window expires, without disabling the account or locking out a username through password-only failures. Ordinary browsing and local recovery remain available. Events use fixed codes for setup, enable/disable, failures, recovery use/replacement, enforcement/reset, and key rotation; the existing 180-day retention applies. Neither logs nor audits include secrets, codes, key bytes, provisioning URIs, passwords, or raw verification errors.

## Encryption keys, backups, and rotation

Schema v8 stores encrypted enrolled/pending secrets separately from the disposable index. Encryption uses RustCrypto's [XChaCha20-Poly1305](https://docs.rs/chacha20poly1305/0.10.1/chacha20poly1305/) with random 24-byte nonces, versioned key IDs, and authenticated account/purpose binding. TOTP generation uses [totp-rs](https://docs.rs/totp-rs/5.7.2/totp_rs/). Secrets cannot be moved between accounts or pending/enrolled purposes without failing authentication.

The keyring is created lazily at first enrollment or enforcement, as an owner-only regular file next to the state database: `agent-lens-state.sqlite` defaults to `agent-lens-state.mfa-keys.json`. Override it with global `--mfa-key-file PATH` or `AGENT_LENS_MFA_KEY_FILE`. Use a trusted, service-writable persistent directory, not a copied agent home. Symlink key files and group/other permissions are rejected on Unix. Never put key material itself in environment variables, logs, source control, or the index. Default keyring and temporary rotation filenames are ignored by Git; also ignore any custom repository-local path.

Back up the **stopped state database and its keyring** as sensitive material. Keep a keyring backup secured separately where practical; a database-only backup cannot restore enrolled authenticators. Missing/unusable keyrings fail closed for existing encrypted state and are never silently replaced. Local `reset-mfa` alone can open the state for recovery without loading the missing key; it does not disclose existing secrets.

Rotate keys with:

```bash
cargo run -- account rotate-mfa-key
# Example explicit deployment paths:
cargo run -- --state-database /var/lib/agent-lens/agent-lens-state.sqlite \
  --mfa-key-file /var/lib/agent-lens/agent-lens-state.mfa-keys.json account rotate-mfa-key
```

Rotation takes the state writer lock, durably publishes a new owner-only keyring retaining previous keys, then transactionally rewraps enrolled and pending ciphertext. If rewrapping fails, old committed ciphertext remains decryptable with retained keys. The running service reloads keys for each protected operation, so rotation does not require a service restart. Retained key versions (maximum 128) support interrupted rotation and older database backups; automated old-key retirement is deliberately absent. Replace old off-host backups according to your retention policy after verifying rotation; do not manually discard keys still needed by retained backups.

## Upgrade and verification

Back up stopped state before upgrading. Schema v8 introduced MFA; current builds migrate through [v10 OIDC identity links](oidc.md#local-administrator-identity-links), preserving encrypted TOTP, recovery digests, accounts, teams, grants, sessions, and tokens. Existing unenrolled users remain optional. OIDC identity changes revoke credentials but do not reset MFA or remove an enrollment requirement. No rebuild or refresh is needed. Restart after deploying; on this host, restart and verify `agent-lens.service` whenever the debug binary is replaced.

Run `bash scripts/check.sh`. Synthetic tests cover known TOTP vectors, drift/replay and concurrent recovery use, encryption binding and malformed envelopes, owner-only keys, pending expiry and session/generation isolation, mandatory enrollment gates, browser login and fresh-factor management, code secrecy in rendered HTML/audits, token/Basic boundaries, optional disable, required reset, persisted verification budgets, legacy audit migration, key rotation with failure recovery, missing-key CLI recovery, and no disposable-index writes by account commands.
