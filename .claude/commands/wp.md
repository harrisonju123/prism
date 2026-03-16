Manage work packages in the PrisM context store (units of work within a plan).

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context wp $ARGUMENTS
```

Common usage:
- `/wp add --plan <plan_id> --title "Implement JWT" --description "..."` — add work package to a plan
- `/wp list --plan <plan_id>` — list work packages for a plan
- `/wp status <id>` — show work package status
- `/wp start <id>` — mark work package as started
- `/wp fail <id> --reason "..."` — mark work package as failed
- `/wp cancel <id>` — cancel a work package

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
