# Runtime Notes

Internal architecture notes for developers working on the ZLUDA fork side of
this repo. End-user documentation lives in `exstar/docs/`; this file is for
contributors touching the Rust source.

- `zluda_redirect` is the EXStar compatibility entrypoint — required at
  runtime for EXStar Hub to launch under this fork.
- New EXStar-specific redirect logic should be isolated under
  `zluda_redirect/src/exstar/` rather than scattered through `lib.rs`.
- Build output lives in `target/debug/` (or `target/release/`); the launcher
  scripts in `exstar/scripts/launch/` find these binaries via repo-relative
  paths — no manual sync required.
- Per-run launcher logs land in `exstar/logs/launcher/`; crash dumps in
  `exstar/dumps/`. Both are gitignored — keep them that way.
