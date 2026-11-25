
pub(crate) const HELP: &str = "strig - run Coolify compose stacks without Coolify (Traefik labels, generated secrets, import/migrate)

USAGE:
    strig <command> [args]

COMMANDS:
    render      Print the final compose YAML of a stack (writes .env.generated)
    up          Render and start a stack with `docker compose up`
    proxy       Start/stop the inner Traefik behind Coolify's Traefik
    route       Make Coolify's Traefik forward a stack's domains to the stack host
    doctor      Check host and stacks for problems
    import      Copy services/applications from a Coolify host
    migrate     Take over generated secrets from a Coolify host
    move        Move one Coolify service/application here in one go (data, up, route)
    ls          List homelab stacks: host, state, service, image, tag, domains (run in the homelab repo or set STRIG_HOME)
    domains     List domains with stack, host and forward-auth
    drift       Compare Git with the hosts (missing, extra, differs)
    update      Check for newer image tags, pin digests, update a stack (pull, deploy, health check)
    deploy      Run Ansible for one stack or host
    help        Show help for a command: `strig help <command>`

OPTIONS:
    -h, --help       Show help (also per command: `strig <command> --help`)
    -V, --version    Show version

A stack is a directory with a compose file; its name (lowercase) is the compose project name.";

pub(crate) fn help_for(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "render" => "strig render <dir> | strig render --all <root>

Renders the compose file of <dir>: fills SERVICE_URL/FQDN/PASSWORD/... variables, adds Traefik
labels, the project name and the external network `proxy`. The YAML goes to stdout.
While an inner Traefik exists (`strig help proxy`) the labels are plain `http` only (no tls/certresolver/redirect).
Side effect: creates/extends <dir>/.env.generated (mode 600); existing values are never changed.

    --all <root>   Render every stack dir below <root>; prints OK/ERROR per stack, exit 1 on any error

Refused if <dir>/.lock exists (its content is the reason).",
        "up" => "strig up <dir> [compose up args]

Renders <dir> into docker-compose.generated.yml and runs `docker compose up -d` with
--env-file .env (if present) and .env.generated.
Extra arguments replace the default `-d` and are passed to `docker compose up`.
Refused if <dir>/.lock exists.",
        "proxy" => "strig proxy up --trust <ip> [--port 8080] [--to <host>] [--dry-run]
strig proxy down [--to <host>]

up    Creates network `proxy` if missing and starts the inner Traefik `stackrig-proxy` (label stackrig.proxy=true)
      that routes the stacks by their labels on plain http, published on --port (default 8080). Run it again to update it.
down  Removes the container.

    --trust <ip>  Address Coolify's Traefik connects from (seen by the inner one); only it may set X-Forwarded-*. Required.
    --to <host>   Stack host (user@host over ssh, or `local`, default). Needs docker there.
    --dry-run     Print the compose file only

While a container with that label exists (running or not), `render`/`up` emit plain http labels (no TLS, no redirect)
because Coolify's Traefik terminates TLS. STRIG_BEHIND_PROXY=1|0 forces the mode. Then: `strig route`.",
        "route" => "strig route <dir> [--from <host>] [--to <host>] [--upstream <url>] [--dest <path>] [--resolver <name>] [--dry-run]

Makes Coolify's Traefik forward the domains of the stack in <dir> to a stack host, by writing
<dest>/stackrig-<stack>.yaml (one file per stack, entryPoint https, certResolver letsencrypt) on the Coolify host.
Coolify's Traefik reloads it by itself. Use together with STRIG_BEHIND_PROXY=1 and an inner Traefik (see README).

    <dir>        Stack directory here (compose file; the domains come from its SERVICE_URL/FQDN)
    --from <h>   Coolify host (user@host over ssh, or `local`, default): the file is written THERE (`sudo -n tee`);
                 the directory is read from the coolify-proxy container
    --to <h>     Stack host (default `local`): the inner Traefik is the container with label stackrig.proxy=true,
                 its published port and this host's address (Linux `ip`) become the upstream
    --upstream   Use this URL (e.g. http://192.168.178.50:8080) instead of looking it up on --to
    --dest       Use this directory on --from instead of the one coolify-proxy reads
    --resolver   certResolver name (default letsencrypt)
    --dry-run    Print the config, write nothing

Refused if <dir>/.lock exists. Writes .env.generated like `render`.",
        "doctor" => "strig doctor [fix] [<dir> | --all <root>]

Checks: Docker and Compose reachable, network `proxy`. With a dir: renders a temp copy and reports
locks, render errors, variables without a value, secrets `up` would newly generate, and .env* readable by others.

    fix          Create network `proxy`, set .env/.env.generated to mode 600
    <dir>        Check one stack
    --all <root> Check every stack dir below <root>

Exit code 1 if errors remain (warnings do not count).",
        "import" | "migrate" => "strig import|migrate coolify <host|local> <target-dir> [--only a,b] [--dry-run]

    import    Writes per Coolify service/application docker-compose.yml (domains filled in) and the
              user's .env into <target-dir>/<name>/. Never overwrites. Sets .lock so render/up refuse
              the stack until `migrate` ran.
    migrate   Takes over the generated SERVICE_* secrets into .env.generated and removes the import
              lock (only the one `import` wrote).

    <host>      user@host over ssh (needs `sudo -n` for docker and /data/coolify), or `local` to run on the Coolify machine
    --only a,b  Only these Coolify services/applications (default: all); unknown names are an error
    --dry-run   Run the real code on a throw-away copy; nothing below <target-dir> changes

Exit code 1 on errors (and for migrate when stacks need attention).",
        "move" => "strig move coolify <name> <target-dir> [--from <host|local>] [--stop] [--trust <ip>] [--dry-run]

Runs on the stack host (here). Steps: stop the Coolify containers (--stop), start the inner Traefik if missing (--trust),
import into <target-dir>/<name> (+ migrate for services), copy named volumes into <stack>_<volume>, `up`, `route` into Coolify's Traefik.

    --from <h>    Coolify host (user@host, or `local` = Coolify runs on this machine, default). Needed if Coolify is elsewhere.
    --stop        Stop the running Coolify containers; without it a still-running service/application aborts the move
    --trust <ip>  Needed only if no inner Traefik exists yet (see `strig help proxy`)
    --dry-run     Print the plan, change nothing (still reads from the Coolify host)

Needs on the Coolify host (if remote): key-based ssh (BatchMode) and `sudo -n` for docker and /data/coolify.
Volumes are copied with `alpine` (docker run), so both hosts need docker and that image.

Coolify's containers stay stopped (not removed): to roll back, `docker compose down` here, delete
stackrig-<stack>.yaml in Coolify's Traefik dir, `docker start` the containers. Afterwards disable the app in Coolify,
or a redeploy there starts it again. Services: volumes are expected as `<uuid>_<key>`.",
        "ls" => "strig ls [<stack>...] [--host <host>...] [--show <columns>]

Read-only. Run in the homelab repo (or set STRIG_HOME). Git is the target state: host_vars/<host>/stacks.yml,
stacks/<name>/{compose.yml,meta.yml}, inventory.yml (ssh target = ansible_user@ansible_host, key-based).

Lists per stack: host, running/all containers, services (name, image, tag), domains (one line per service/domain, the other columns only on the first).

    <stack>...     Only these stacks (several allowed)
    --host <host>  Only stacks on this host, repeatable (other hosts are not contacted)
    --show <cols>  Comma list of columns, in this order: stack, host, state, service, image, tag, domains (default: all).
                   With --show, rows are sorted by these columns in this order (empty last); by default stacks by name, services in compose order. Only `state` needs SSH; without it no host is contacted. E.g. `--show stack,domains`.

Stacks are OR-ed, hosts are OR-ed, both together AND-ed. Any unknown stack/host (or a stack not on the given
hosts) is an error: exit 1, nothing printed. Hosts that are not reachable over SSH are skipped with a warning.",
        "domains" => "strig domains [<stack>...] [--host <host>...]

Read-only. Run in the homelab repo (or set STRIG_HOME). Reads host_vars/<host>/stacks.yml and stacks/<name>/meta.yml.

Lists per domain: stack, host, forward-auth (meta.yml `auth` / `auth_domains`). Needs no SSH access.

    <stack>...     Only these stacks' domains (several allowed)
    --host <host>  Only domains of stacks on this host, repeatable

Stacks are OR-ed, hosts are OR-ed, both together AND-ed. Any unknown stack/host (or a stack not on the given
hosts) is an error: exit 1, nothing printed.",
        "drift" => "strig drift

Read-only. Run in the homelab repo (or set STRIG_HOME). Git is the target state: host_vars/<host>/stacks.yml,
stacks/<name>/compose.yml, inventory.yml (ssh target = ansible_user@ansible_host, key-based).

Compares Git with what runs on each host; prints one line per finding, or `no drift`:
    missing   stack is in Git but has no containers on the host
    extra     stack is running on the host but not in Git
    differs   ~/homelab/stacks/<n>/compose.yml on the host is missing or != compose.yml in Git (only compose.yml is compared)

Exit 1 on findings. Hosts that are not reachable over SSH are skipped with a warning.",
        "update" => "strig update [-i|--interactive] [--major] --check [<stack>|--all] | strig update --pin <stack>|--all | strig update <stack>|--all

    --check            Show newer tags (same format: prefix, suffix, number of parts) and the registry digest of `latest`. Writes nothing.
    -i, --interactive  Interactive (plain update only): per image `Y` apply, `n` keep, a tag to use that version instead (checked in
                       the registry), `q` abort (nothing written). Images without a newer tag can be changed by typing a tag, too.
                       Afterwards asks before deploying. Prompts go to stderr.
    --major            Also take major jumps. Default: stay in the same major (for 0.x the first non-zero part counts: 0.21.x stays 0.21.x);
                       a higher one is only shown as `(major available: ...)`.
    --pin              Rewrite `latest`/untagged images to repo:<tag>@sha256:<digest> (tag found by digest among the 20 newest
                       plain version tags; else repo:latest@sha256:...). Edits compose.yml only, you commit.
    <stack>|--all      Bump fixed tags to the newest one in compose.yml, `docker pull` all images on the host, `strig deploy`
                       (Ansible), then wait up to 120 s for health: Docker healthcheck, else running without restarts.
                       Exit 1 on failure; no automatic rollback (git revert + `strig deploy`). No backup yet.
                       Images with `${VAR}` are skipped; a downgrade (typed tag older than current) is not reported.

Registry access: `crane` if installed, else curl (+ `docker buildx imagetools` for digests). Run in the homelab repo or set STRIG_HOME.",
        "deploy" => "strig deploy <stack> | --host <host> [ansible-playbook flags]

Runs `ansible-playbook site.yml --limit <host> [-e only=<stack>]` in the homelab repo (or STRIG_HOME).
The host comes from host_vars. Extra flags go to Ansible, e.g. `strig deploy beszel --check --diff`.",
        _ => return None,
    })
}

/// Handles `help`, `-h/--help`, `-V/--version`; returns the exit code if one applied.
pub(crate) fn help_main(a: &[String]) -> Option<i32> {
    let cmd = a.get(1).map(String::as_str);
    let ask = |t: Option<&str>| match t.and_then(help_for) { Some(h) => { println!("{h}"); 0 } None => { println!("{HELP}"); 0 } };
    match cmd {
        None => { eprintln!("{HELP}"); Some(2) }
        Some("-V" | "--version") => { println!("strig {}", env!("CARGO_PKG_VERSION")); Some(0) }
        Some("-h" | "--help") => Some(ask(None)),
        Some("help") => match a.get(2).map(String::as_str) {
            Some(t) if help_for(t).is_none() => { eprintln!("error: unknown command `{t}`\n\n{HELP}"); Some(2) }
            t => Some(ask(t)),
        },
        Some(c) if a[2..].iter().any(|x| x == "-h" || x == "--help") && help_for(c).is_some() => Some(ask(Some(c))),
        _ => None,
    }
}
