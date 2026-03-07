Run a uglyhat CLI command using the correct Rust binary (`~/.cargo/bin/uh`).

**IMPORTANT:** Always invoke `~/.cargo/bin/uh` — never bare `uh`. The Homebrew `uh` at `/opt/homebrew/bin/uh` is the old Go CLI that requires an HTTP server and returns 401 errors.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/uh $ARGUMENTS
```

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes (e.g. if `.uglyhat.json` is missing, run `~/.cargo/bin/uh init "Project Name"`).

Common commands:
- `~/.cargo/bin/uh next` — show unblocked, unclaimed tasks
- `~/.cargo/bin/uh context` — full workspace overview (agents, tasks, stale)
- `~/.cargo/bin/uh task claim <id> --name <agent>` — claim a task before starting
- `~/.cargo/bin/uh task update <id> --status done` — mark task complete
- `~/.cargo/bin/uh checkin --name <agent>` — register session (auto-runs via hook)
- `~/.cargo/bin/uh checkout --name <agent> --summary "..."` — end session
- `~/.cargo/bin/uh agents` — show all agent statuses
