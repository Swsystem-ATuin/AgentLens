# Service management and scheduled refresh

The files under `packaging/` run an installed `/usr/local/bin/agent-lens` as a
dedicated `agent-lens` account. Both init systems use the same administrator-owned
`/etc/agent-lens/agent-lens.env` file. The supplied defaults keep the HTTP listener
on loopback, the disposable index and persistent account state under
`/var/lib/agent-lens`, and copied source data under
`/var/lib/agent-lens/data`.

Build and install the binary, account, directories, and configuration first:

```bash
cargo build --release --locked
sudo install -m 0755 target/release/agent-lens /usr/local/bin/agent-lens
sudo useradd --system --home-dir /var/lib/agent-lens --shell /usr/sbin/nologin agent-lens
sudo install -d -o agent-lens -g agent-lens -m 0700 /var/lib/agent-lens
sudo install -d -o root -g agent-lens -m 0750 /etc/agent-lens
sudo install -o root -g agent-lens -m 0640 packaging/agent-lens.env.example \
  /etc/agent-lens/agent-lens.env
```

If the system account already exists, omit `useradd`. Copy or mount the raw agent
homes at the configured `AGENT_LENS_DATA` location and grant the service account
read and directory-traversal access. Agent Lens never writes to that tree. Keep
the environment file root-owned because the SysV scripts source it as shell
syntax; use only simple `KEY=value` entries like the example.

`AGENT_LENS_BIND` and `AGENT_LENS_LOCAL` configure the server. The shipped values
use authenticated mode on `127.0.0.1:3001`. Before starting that mode, provision
the first account as the service user:

```bash
sudo install -o agent-lens -g agent-lens -m 0600 /tmp/agent-lens-password \
  /var/lib/agent-lens/account-password
sudo -u agent-lens sh -c 'set -a; . /etc/agent-lens/agent-lens.env; \
  exec /usr/local/bin/agent-lens account create --username alice \
  --password-file /var/lib/agent-lens/account-password'
sudo rm /var/lib/agent-lens/account-password
```

Alternatively, set `AGENT_LENS_LOCAL=true` for passwordless, loopback-only
personal use. Set `AGENT_LENS_SECURE_COOKIES=true` when an HTTPS reverse proxy is
the only route to the service. Optional project configuration belongs outside the
raw source tree and can be selected with `AGENT_LENS_PROJECTS_FILE`.

## systemd

Install and activate the web service and refresh timer:

```bash
sudo install -m 0644 packaging/systemd/agent-lens.service /etc/systemd/system/
sudo install -m 0644 packaging/systemd/agent-lens-refresh.service /etc/systemd/system/
sudo install -m 0644 packaging/systemd/agent-lens-refresh.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now agent-lens.service agent-lens-refresh.timer
```

The timer starts one minute after boot and schedules refresh once per minute.
systemd does not start a second instance while its one-shot refresh service is
still active. `Persistent=true` triggers a missed run after downtime. Inspect the
schedule and logs with:

```bash
systemctl list-timers agent-lens-refresh.timer
journalctl -u agent-lens.service -u agent-lens-refresh.service
```

After changing the environment file, restart the web service. The next timer run
loads refresh settings afresh:

```bash
sudo systemctl restart agent-lens.service
sudo systemctl start agent-lens-refresh.service
```

## SysV init and cron

The SysV example requires the `start-stop-daemon` helper. The refresh wrapper
also requires `flock`, normally supplied by util-linux. Install the scripts and
cron file with their required modes:

```bash
sudo install -m 0755 packaging/sysvinit/agent-lens /etc/init.d/agent-lens
sudo install -m 0755 packaging/sysvinit/agent-lens-refresh \
  /usr/local/sbin/agent-lens-refresh
sudo install -m 0644 packaging/sysvinit/agent-lens-refresh.cron \
  /etc/cron.d/agent-lens-refresh
sudo install -m 0644 packaging/sysvinit/agent-lens.logrotate \
  /etc/logrotate.d/agent-lens
sudo update-rc.d agent-lens defaults
sudo service agent-lens start
```

Use the platform's equivalent of `update-rc.d` where necessary. Cron requests a
refresh every minute; the wrapper takes a non-blocking lock, so it skips a tick
instead of overlapping an earlier refresh. Cron output is handled by the host's
normal cron mail or logging configuration. The web service writes sanitised
process output to `/var/log/agent-lens/agent-lens.log`; configure the host's
logrotate service to use the included bounded rotation policy. Run an immediate
refresh with:

```bash
sudo -u agent-lens /usr/local/sbin/agent-lens-refresh
```

Refresh is safe while the web server reads the index: readers retain a consistent
SQLite snapshot and see the new projection after commit. Only one importer should
run at a time. A full `rebuild` is different: stop the web service first, run the
rebuild using the same environment, then restart it.

## Changing the interval or paths

For systemd, copy the timer interval into an override rather than editing the
shipped unit. For example, `sudo systemctl edit agent-lens-refresh.timer` can set
another `OnUnitActiveSec`; run `daemon-reload` and restart the timer afterward.
For cron, change the schedule in `/etc/cron.d/agent-lens-refresh`.

Keep the database, data, and bind settings in the shared environment file. CLI
arguments override the corresponding `AGENT_LENS_DB`, `AGENT_LENS_STATE_DB`,
`AGENT_LENS_DATA`, `AGENT_LENS_BIND`, `AGENT_LENS_LOCAL`,
`AGENT_LENS_PROJECTS_FILE`, and `AGENT_LENS_SECURE_COOKIES` values.
