Manage plans in the PrisM context store (create, list, show, approve work packages).

Note: named `plan-ctx` to avoid conflict with Claude Code's built-in `/plan` command.

Run the following command and display the result:

```bash
export PATH="$HOME/.cargo/bin:$PATH" && ~/.cargo/bin/prism context plan $ARGUMENTS
```

Common usage:
- `/plan-ctx create --intent "Refactor auth module"` — create a new plan
- `/plan-ctx list` — list all plans
- `/plan-ctx show <plan_id>` — show plan details
- `/plan-ctx approve <plan_id>` — approve a plan for execution

Parse and display JSON output in a readable format. If the command fails, show the error and suggest fixes.
