# Third-party notices

LogFilter uses third-party libraries and assets. The project-level `GPL-3.0-or-later` license does not replace their copyright notices or licenses. Dependency versions are recorded in `Cargo.lock` and `pnpm-lock.yaml`; the effective dependency set should be reviewed for each release.

This file currently records the separately bundled font asset; it is not a complete per-package license report for a binary distribution. Before publishing a formal installer, the release process must generate and bundle a license inventory from the locked Rust and pnpm dependency sets and review the actual platform artifact.

## Geist Variable font

The application bundles the Geist Variable font through `@fontsource-variable/geist`.

- Copyright 2024 The Geist Project Authors
- License: SIL Open Font License 1.1 (`OFL-1.1`)
- License text: [`third-party-licenses/Geist-OFL-1.1.txt`](third-party-licenses/Geist-OFL-1.1.txt)
- Upstream: [vercel/geist-font](https://github.com/vercel/geist-font)

The font remains licensed under the OFL and is not relicensed under the GPL. It is currently bundled without modification.
