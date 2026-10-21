## Workbench (task board)

This repo is linked to a Workbench project through `.workbench.json` (`{ "project": "NEW" }`). **The board is the source of truth for what to work on.**

Use the `workbench` MCP server and pass your working directory as `cwd`, or the project key as `project`, on every call.
**This repo's project is only the default.** When the user names a project or product area ("in Outrouted business", "in the
ops project"), call `list_projects` first and pass the matching `project` NEW; a name match beats this repo's default.

- **Claude Code** can fall back to the `workbench` CLI.
- **Codex** uses MCP only: its sandbox has no network for the CLI.
- Edit the Markdown mirror (`~/.workbench/export/NEW/ID.md`) only as a last resort.

### Loop

1. `next_task` returns the highest-priority task you may run alone. Resume anything already `doing` first. If nothing is runnable, say so and list what waits on the user.
2. Call `get_task {id, full: true}` and `get_prompt {id}`. Read every prerequisite's artifacts. A `{{X_OUTPUT}}` token means that task's output.
3. Call `set_status {id, status: "doing"}`. This is a claim: if it returns 409, someone else took the task, so call `next_task` again. Keep one task in progress unless told otherwise.
4. Do the work. Follow the task's requirements and mode (`/plan` is read-only, `/goal` implements). Do not:
   - deploy or write to production
   - make payments or bookings
   - use credentials
   - make uncontrolled paid API calls
   - refactor unrelated code
5. Call `attach_artifact` with the durable output: an audit, spec or decision as Markdown, or a PR link. Record evidence: files changed, commands run, numbers. For a file in the repo pass `sourcePath` (Workbench copies it in) rather than pasting it into `content`.
6. Check every clause of **Done when** against that evidence and write the check into the artifact.
7. Call `set_status {id, status: "review"}`. A lead agent (you, when the user put you in charge) approves Review work with `set_status {status: "done", note: "<verdict>"}` or sends it back with `todo` and a note. A 403 `owner_approval` means the user approves that one: leave it in Review and move on. Never wait on approvals.
8. If your findings change a later task, `update_task` it and `add_note` saying what changed and why.

### Stop and ask

Call `set_status {id, status: "blocked", reason}` with a specific reason, and tell the user, when you need any of these:

- a manual action
- credentials
- sign-off on spending, production or an irreversible step
- spending
- a production action
- an ambiguous product or business decision

Never run tasks assigned to ME, ChatGPT or Claude Design. Never skip a blocked task silently or invent a missing decision.

### Before you stop

Don't leave a task in `doing`. Hand it back as `todo` with a note, or set `blocked` with a reason.
