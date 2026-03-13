---
description: Build, Tag and Push a new Release version
---

1. Ensure version is updated in `Cargo.toml`, `CHANGELOG.md`, `README.md`, and `DEVELOPMENT_REPORT.md`
2. Run cargo check to ensure everything is correct
// turbo
3. Commit and tag the new version
```bash
git add .
git commit -m "chore: release v0.10.43"
git tag v0.10.43
```
// turbo
4. Push to main and nightly branches
```bash
git push origin main
git push origin nightly
git push origin v0.10.43
```
