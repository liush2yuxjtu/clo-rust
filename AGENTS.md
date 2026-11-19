# CLO Rust repository workflow

- Work directly on `main`. Keep GitHub Issues and Pull Requests disabled. Do not open or use issues or PRs for project work.
- Keep GitHub Actions disabled. Do not add CI workflows or require CI checks. Keep existing test and smoke scripts available for local runs.
- Before every commit and push, follow the relevant checks in the project [`/verify` skill](.agents/skills/verify/SKILL.md). Record the commands, results, and verdict. Resolve a failed or blocked required check before shipping.
- Build, verify, and publish release artifacts manually. Verify each artifact on its target platform and publish its SHA-256 checksum.
