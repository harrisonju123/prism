Manage PrisM context threads (named context buckets that group related memories, decisions, and sessions).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context thread $ARGUMENTS
```

Common usage:
- `/thread list` — list active threads
- `/thread list --archived` — include archived threads
- `/thread create auth-refactor --desc "JWT auth migration" --tags auth,security` — create a new thread
- `/thread archive auth-refactor` — mark a thread done
- `/thread guard <thread-name> <rule>` — add a guardrail to a thread

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
