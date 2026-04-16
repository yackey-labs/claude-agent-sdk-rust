# Upstream tracking

This crate is a port of the Python [`claude-agent-sdk`](https://github.com/anthropics/claude-agent-sdk-python).

The fields below describe the upstream commit this port currently corresponds to. Update them whenever
you sync upstream changes (the `/sync-upstream` skill does this automatically).

```
upstream_repo: https://github.com/anthropics/claude-agent-sdk-python
upstream_commit: aaac538e0c2f3a5270cb91e31940630a3d454405
upstream_short: aaac538
upstream_subject: chore: bump bundled CLI version to 2.1.110
upstream_committed: 2026-04-15T22:06:53Z
synced_at: 2026-04-15
synced_by: initial port
```

## How to update

1. Run the `/sync-upstream` skill (see `.claude/skills/sync-upstream/SKILL.md`).
2. Or manually:
   ```bash
   cd /tmp && git clone https://github.com/anthropics/claude-agent-sdk-python
   cd claude-agent-sdk-python && git log --oneline <last-synced-commit>..HEAD -- src/
   ```
   then port the changes and bump `upstream_commit` here.
