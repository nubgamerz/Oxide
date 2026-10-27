## What and why

<!-- What does this change, and why? A sentence or two is plenty. -->

## Issue

<!-- "Closes #123". Anything bigger than a small fix should have an issue first. -->

## How it was tested

<!-- What you tried, and the result of the quick tier:
     pwsh -NoProfile -File scripts/check.ps1 -Tier quick -->

## Checklist

- [ ] `cargo fmt` is clean
- [ ] The quick tier passes locally (`scripts/check.ps1 -Tier quick`)
- [ ] A new user-facing operation has its MCP action (see CONTRIBUTING.md, "New features need two more things")
- [ ] The manual page is updated if users will see the change (`docs/manual/README.md`)
- [ ] Release-notes lines are added to `docs/releases/<next version>.md` if it ships
- [ ] I agree to the contribution terms in [CONTRIBUTING.md](https://github.com/nubgamerz/Oxide/blob/master/CONTRIBUTING.md#licence) (my contribution under the GPLv3, plus the right for the maintainer to relicense it)
