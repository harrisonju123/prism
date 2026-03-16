Save a persistent memory (key-value fact) to the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context remember $ARGUMENTS
```

Common usage:
- `/remember auth_approach "Using JWT with refresh tokens"` — global memory
- `/remember auth_approach "Using JWT" --thread auth-refactor` — scoped to a thread
- `/remember db_url "postgres://..." --tags infra,config` — with tags

Memories upsert on (workspace, key) — saving the same key updates it in place.

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
