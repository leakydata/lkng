# Upstream issue drafts

Ready to post to the Freenet project. Not filed yet — they'd go up under
the repo owner's GitHub identity, so they wait for an explicit go-ahead.

Post with:

```bash
gh issue create --repo freenet/freenet-core \
  --title "$(head -1 01-update-non-hosting.md | sed 's/^# //')" \
  --body-file 01-update-non-hosting.md
```

| Draft | Repo | Severity |
| --- | --- | --- |
| `01-update-non-hosting.md` | freenet-core | **High** — blocks multi-writer contracts for LKNG |
| `02-fdev-workspace-root.md` | freenet-core | Low — papercut, one-line workaround |
| `03-license-request.md` | harvest, ghostkeys | Low — patterns usable, code isn't |
