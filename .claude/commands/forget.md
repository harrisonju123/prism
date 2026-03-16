Delete a memory from the PrisM context store by key.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context forget $ARGUMENTS
```

Common usage:
- `/forget auth_approach` — delete the memory with key "auth_approach"

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
