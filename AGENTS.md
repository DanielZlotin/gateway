# 🔧 Gateway development

1. Commit code or documentation changes first, then bump the gateway patch version in `Cargo.toml` and `Cargo.lock` in a separate version-only commit titled exactly `vX.Y.Z`, similar to `npm version patch`. Push both commits to `master`; do not fold the bump into the change commit or use a pre-commit hook that does so.
