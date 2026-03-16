List all agents and their current status in the PrisM context store.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context agents $ARGUMENTS
```

Shows agent roster: name, session open/idle, current thread. No arguments required for basic listing.

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
