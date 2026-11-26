# Security policy

AgentLens indexes potentially sensitive agent transcripts and provides local and
authenticated web access. Authentication or source-grant bypasses, disclosure of
private evidence, unsafe file access, script injection, and credential exposure
are examples of security concerns to report privately.

## Supported versions

Security fixes target the latest published release and the current `develop`
branch. Older releases are not maintained separately; upgrade to the latest
release. Development builds may contain changes that have not shipped in a
release. This policy does not promise a response or remediation deadline.

## Report a vulnerability privately

Use GitHub's [private vulnerability report form](https://github.com/Swsystem-ATuin/AgentLens/security/advisories/new).
You can also find **Report a vulnerability** on the repository's
[Security advisories page](https://github.com/Swsystem-ATuin/AgentLens/security/advisories)
when private reporting is enabled. The report is shared privately with repository
security maintainers rather than published as an issue.

If that form is unavailable, open a public issue titled **Private security contact
requested** containing only a request for a private channel. Do not include the
vulnerability, affected private data, exploit, or credentials in that issue. A
maintainer will need to arrange a private follow-up before you send details.
There is no published project security email address.

Include in the private report:

- Affected release or commit and relevant deployment mode (local or authenticated).
- A description of the impact and any required permissions or configuration.
- Reproduction steps using a minimal synthetic example where possible.
- Expected and observed behavior, and any mitigation or proposed fix.

Do not attach real agent homes, full databases, production transcripts, passwords,
tokens, MFA secrets, or private source code. Redact identifying values even in a
private report. Test only on systems and data you own or have permission to use.

Maintainers will assess the report, discuss reproduction and mitigation privately,
and coordinate publication of an advisory or fix when appropriate. Avoid public
disclosure of exploit details while a report is being assessed. There is no
bug-bounty program or guaranteed reward.

## Deployment guidance

The personal launcher binds an account-free server to loopback; it is intended
for a user's own machine. Use [authenticated access](docs/application-authentication.md)
and HTTPS for shared deployments. Keep raw sources private, keep durable account
state separate from the rebuildable index, and review reports before sharing.
See [import redaction](docs/import-masking.md) for masking options, and the [README](README.md) for supported commands and deployment details.
