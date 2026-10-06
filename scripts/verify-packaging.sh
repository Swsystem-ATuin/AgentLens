#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

sh -n packaging/sysvinit/agent-lens packaging/sysvinit/agent-lens-refresh
# shellcheck disable=SC1091
(set -a; . packaging/agent-lens.env.example; set +a; \
    test "$AGENT_LENS_DATA" = /var/lib/agent-lens/data)
test -x packaging/sysvinit/agent-lens
test -x packaging/sysvinit/agent-lens-refresh
if command -v shellcheck >/dev/null 2>&1; then
    shellcheck packaging/sysvinit/agent-lens \
        packaging/sysvinit/agent-lens-refresh scripts/verify-packaging.sh
fi
if command -v systemd-analyze >/dev/null 2>&1; then
    systemd-analyze verify \
        packaging/systemd/agent-lens.service \
        packaging/systemd/agent-lens-refresh.service \
        packaging/systemd/agent-lens-refresh.timer
fi

grep -Fxq 'ExecStart=/usr/local/bin/agent-lens serve' packaging/systemd/agent-lens.service
grep -Fxq 'ExecStart=/usr/local/bin/agent-lens refresh' packaging/systemd/agent-lens-refresh.service
grep -Fxq '* * * * * agent-lens /usr/local/sbin/agent-lens-refresh' \
    packaging/sysvinit/agent-lens-refresh.cron
grep -Fq '/var/log/agent-lens/agent-lens.log {' \
    packaging/sysvinit/agent-lens.logrotate
