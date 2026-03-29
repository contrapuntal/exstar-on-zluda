# Runtime Notes

- This repo is the primary development home for source changes.
- `zluda_redirect` remains required for the current working EXStar path.
- EXStar-specific redirect code should be isolated over time under `zluda_redirect/src/exstar/`.
- Build output in `target/debug/` serves as the binary staging area (replaces the old `applications/` directory from the monorepo).
- Launch scripts in the sibling `exstar-on-zluda` repo discover binaries here via relative path.
- Do not store launcher logs, screenshots, or dumps here — those belong in `exstar-on-zluda/logs/`.
