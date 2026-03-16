Register this agent session with the PrisM context store (session start).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context checkin $ARGUMENTS
```

Common usage:
- `/checkin --name claude` — checkin with default name
- `/checkin --name claude-zed-surface --capabilities rust,api` — checkin with capabilities
- `/checkin --name claude --thread auth-refactor` — checkin into a specific thread

Returns active threads, global memories, recent sessions, and other agents. Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
