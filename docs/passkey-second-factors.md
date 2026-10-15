# Opt-in passkey second factors

Passkeys are **optional second factors, never passwordless sign-in**. Every passkey sign-in starts by verifying the account password. Users may enroll passkeys alongside TOTP, use recovery codes, and register up to ten named credentials. Existing administrator-required MFA accepts either an enrolled passkey or TOTP. Administrator/CLI MFA reset removes both kinds while preserving the enrollment requirement.

## Configure before enabling

Without a configured public URL, passkey ceremonies are disabled and no enrollment/sign-in control is enabled. Configure one exact HTTPS origin and Secure cookies before offering passkeys:

```bash
cargo run -- serve --secure-cookies --public-url https://lens.example.org
# Equivalent service settings:
# AGENT_LENS_PUBLIC_URL=https://lens.example.org
# AGENT_LENS_SECURE_COOKIES=true
```

Use the real deployment hostname, not the example. Paths, query strings, fragments, embedded credentials, HTTP URLs, IP-literal RP hosts, and local unauthenticated mode are rejected. A non-default HTTPS port can be configured explicitly. The relying-party ID is the exact configured DNS hostname, not a broader parent domain. Origin comparison includes the configured scheme/port and does not allow subdomains or arbitrary ports. `Host`, `Forwarded`, and `X-Forwarded-*` never configure or expand trust. Terminate HTTPS at a trusted reverse proxy, prevent direct public backend access, and forward the same-origin requests unchanged.

Changing hostname/origin requires a recovery/re-enrollment plan: stored credentials retain their original origin and cannot silently migrate. Restore the original configuration or recover/reset and enroll new credentials. Removing configuration does **not** convert an enrolled account to password-only access; TOTP/recovery or administrator reset remains necessary. This host's URL is intentionally left unconfigured until an operator supplies its real public HTTPS URL.

## User and administrator workflow

Open Account → MFA → Manage opt-in passkey second factors at `/account/passkeys`. Choose a device name and confirm your password. If MFA is already enrolled, also confirm an existing factor or recovery code before adding another device. On a compatible HTTPS browser, confirm its authenticator prompt and device PIN/biometrics. User verification is required; presence-only keys are not accepted. First-factor enrollment shows ten one-use recovery codes only in the completion response. Adding a device to an already enrolled account preserves the existing recovery set.

At sign-in, enter username/password and choose “Sign in with password and passkey.” The password is checked before a credential allowlist is returned. Username-less and discoverable-credential sign-in are not offered; resident storage is not required. Authenticators may nevertheless store/sync credentials according to their own platform policies. Browser JavaScript only converts WebAuthn binary fields and forwards responses to same-origin endpoints; it never sends credentials to an external service.

On password-confirmed account/admin forms, “Verify a passkey second factor” fills a one-use, session-bound factor proof. Submit the intended action within 90 seconds. Starting another challenge/proof in that browser context supersedes the previous one. TOTP setup confirmation still requires its actual TOTP code, not a passkey proof.

The passkey page lists at most ten escaped device names and creation timestamps. Removal requires the current password and another valid factor assertion or recovery code. A required account cannot remove its last enrolled factor; it can first enroll another passkey/TOTP or use administrator recovery. Disabling all optional MFA through the MFA page removes passkeys, TOTP, and recovery digests. Adding/removing credentials invalidates other browser sessions and tokens, rotates the current session, and retains its verified second-factor state. Recovery-code replacement and administrator reset keep their existing revocation behavior.

Required-but-unenrolled users may access either TOTP or configured passkey setup and sign-out, but no application data, token creation, or administrative mutation. MFA accounts cannot use password-only metrics Basic authentication; scoped tokens remain the automation boundary.

## Validation, replay, and bounds

Protocol validation uses the maintained [webauthn-rs API](https://docs.rs/webauthn-rs/0.5.5/webauthn_rs/): credential signatures, exact origin/RP, challenge binding, presence, and user verification are validated by the library. Attestation is not requested; no biometric, authenticator private key, or trust-manufacturer policy is collected. Credential IDs are globally unique in SQLite, and duplicate registrations fail atomically.

Registration/authentication state remains exclusively in bounded server memory; state-serialization escape hatches are not enabled. Challenges expire after five minutes, bind account/generation/session/purpose, are consumed once, and do not survive restarts. There is at most one outstanding challenge/proof per browser context and 1,024 across the process. Registration finishes recheck the current account, generation, active session, and device count under the SQLite writer lock. Authentication finishes recheck enabled state, generation, current credential enrollment, and active management session before committing credential updates. Individual deletion, account disablement, password reset, and MFA reset therefore invalidate stale ceremonies.

Stored signature counters are checked against committed state as well as the library's challenge snapshot, preventing out-of-order assertions from lowering a nonzero counter. Backup state is updated using the library's credential API. Synced passkeys commonly use zero counters: zero-to-zero is supported, and counters alone cannot prove whether a synced credential was cloned. Nonzero counter regressions fail closed and may require another factor or recovery after a device reset.

Password confirmations share the existing 60-attempt/minute process limit. Assertion challenges also consume the persisted ten-factor-attempt/five-minute account budget after password confirmation. Verification never disables accounts automatically. All mutation endpoints require CSRF; JSON ceremony requests additionally require the exact configured `Origin`. Bearer tokens cannot use this browser surface. Public-key material stays in the persistent state database, not the disposable index; logs/audits contain fixed lifecycle events, not password, assertion, challenge state, private key, biometric, or proof values.

## Deployment, recovery, and verification

WebAuthn adds an OpenSSL dependency: native builds need OpenSSL development headers and `pkg-config`, and deployment needs the matching OpenSSL runtime. The container builder installs `libssl-dev`/`pkg-config`; the Debian runtime installs `libssl3`.

Back up stopped persistent state and the existing TOTP encryption keyring before upgrading. Schema v9 adds public passkey credentials and audit codes while preserving TOTP secrets, recovery digests, accounts, teams, grants, sessions, and tokens. No source refresh or index rebuild is required. Restart after deploying; this host's `agent-lens.service` must be restarted and verified active whenever the debug binary is replaced.

For lost authenticators, first try another enrolled device, TOTP, or a recovery code. Existing administrator reset and local recovery remove all second factors without dropping mandatory enrollment:

```bash
cargo run -- account reset-mfa --username analyst
```

Run `bash scripts/check.sh` and `node --check assets/passkeys.js` (when Node is available). Normal Rust tests use a software authenticator producing real signed registration/assertion responses. They cover UV, exact origin, signature rejection, challenge expiry/replay, session binding, counter races, revocation, optional/required enrollment, recovery, disabled configuration, restart behavior, memory bounds, HTML escaping, JSON browser flows, and migration preserving existing encrypted TOTP. Physical device/browser prompt compatibility should also be checked against the configured deployment before rollout.
