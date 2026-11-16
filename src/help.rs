
pub(crate) const HELP: &str = "strig - run Coolify compose stacks without Coolify (Traefik labels, generated secrets, import/migrate)

USAGE:
    strig <command> [args]

COMMANDS:
    render      Print the final compose YAML of a stack (writes .env.generated)
    up          Render and start a stack with `docker compose up`
    doctor      Check host and stacks for problems
    import      Copy services/applications from a Coolify host
    migrate     Take over generated secrets from a Coolify host
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
Side effect: creates/extends <dir>/.env.generated (mode 600); existing values are never changed.

    --all <root>   Render every stack dir below <root>; prints OK/ERROR per stack, exit 1 on any error

Refused if <dir>/.lock exists (its content is the reason).",
        "up" => "strig up <dir> [compose up args]

Renders <dir> into docker-compose.generated.yml and runs `docker compose up -d` with
--env-file .env (if present) and .env.generated.
Extra arguments replace the default `-d` and are passed to `docker compose up`.
Refused if <dir>/.lock exists.",
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
