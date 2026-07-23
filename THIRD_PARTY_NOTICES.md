# Third-party notices

LogFilter uses third-party libraries and assets. The project's proprietary license does not replace their copyright notices or licenses. Dependency versions are recorded in `Cargo.lock` and `pnpm-lock.yaml`; the effective dependency set must be reviewed for each release.

Installers bundle the following generated inventories, including dependency versions, declared licenses, upstream links, and available license or notice texts:

- [`third-party-licenses/Rust-dependencies.html`](third-party-licenses/Rust-dependencies.html), generated from the non-development Cargo dependency graph with `cargo-about`.
- [`third-party-licenses/pnpm-production-dependencies.txt`](third-party-licenses/pnpm-production-dependencies.txt), generated from `pnpm licenses list --prod`.

Maintainers regenerate both reports with `pnpm licenses:generate` after dependency-lock changes and before publishing installers. The Rust report uses `cargo-about` 0.9.1 (`cargo install --locked --version 0.9.1 --features cli cargo-about`) and [`about.toml`](about.toml). Both generators reject licenses outside their reviewed allowlists. The generated reports are conservative inventories; release verification must still confirm that they are present in each platform artifact.

## Geist Variable font

The application bundles the Geist Variable font through `@fontsource-variable/geist`.

- Copyright 2024 The Geist Project Authors
- License: SIL Open Font License 1.1 (`OFL-1.1`)
- License text: [`third-party-licenses/Geist-OFL-1.1.txt`](third-party-licenses/Geist-OFL-1.1.txt)
- Upstream: [vercel/geist-font](https://github.com/vercel/geist-font)

The font remains licensed under the OFL and is not relicensed under the project's proprietary terms. It is currently bundled without modification.
