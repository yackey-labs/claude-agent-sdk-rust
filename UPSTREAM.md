# Upstream tracking

This crate is a port of the Python [`claude-agent-sdk`](https://github.com/anthropics/claude-agent-sdk-python).

The fields below describe the upstream commit this port currently corresponds to. Update them whenever
you sync upstream changes (the `/sync-upstream` skill does this automatically).

```
upstream_repo: https://github.com/anthropics/claude-agent-sdk-python
upstream_commit: 50058168e34662c52169c919cc20362433a62e38
upstream_short: 5005816
upstream_subject: docs: update changelog for v0.1.79
upstream_committed: 2026-05-09T00:23:16Z
synced_at: 2026-05-09
synced_by: claude-code /sync-upstream
```

## How to update

1. Run the `/sync-upstream` skill (see `.claude/skills/sync-upstream/SKILL.md`).
2. Or manually:
   ```bash
   cd /tmp && git clone https://github.com/anthropics/claude-agent-sdk-python
   cd claude-agent-sdk-python && git log --oneline <last-synced-commit>..HEAD -- src/
   ```
   then port the changes and bump `upstream_commit` here.
