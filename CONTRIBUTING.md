# Contributing

Thanks for contributing to Plow SMI. This project is maintained by
[Infervisor](https://infervisor.ai).

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Prerequisites

- Rust toolchain (stable, edition 2021)
- Optional: [Nix](https://nixos.org/download/) with flakes enabled for a
  reproducible dev shell: `nix develop`

No GPU or vendor SDK is required to build or test. GPU vendor libraries
(`libnvidia-ml`, `libze_loader`) are loaded at runtime via
`dlopen` — `plows-gpu` discovers them if present and skips them cleanly if
not, so CI and laptops without GPUs stay green.

## Build and test

```bash
cargo build --workspace --release
cargo test  --workspace
```

Or via Nix:

```bash
nix build            # unified plow-smi CLI
nix build .#all      # every binary in one derivation
nix run .#plows-top   # run a single binary directly
nix flake check       # build + test every package
```

## Pull requests

- Keep changes focused: one problem per PR when practical.
- Match existing style and patterns in the crates you touch.
- Add or update tests for behavioral changes.
- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets` before
  submitting.
- Prefer short, clear PR descriptions: what changed and why.

## LLM-assisted contributions

Much of this codebase was written with LLM coding agents, and we expect
contributions to be too. Using one is welcome and does not need to be hidden.

What does not change is accountability:

- **A named human is accountable for every merge to `main`.** Not the agent
  that wrote the patch, and not the agent that reviewed it. If it lands and it
  breaks, a person owns that.
- **The submitter is accountable for the contents of their PR** — they must
  understand the change well enough to defend it in review and to fix it when
  it fails. "The agent wrote it" is not an explanation of why a change is
  correct.
- **Gates are evidence, not decoration.** Where a change claims a gate passed
  (build, tests, `nix flake check`), paste what the gate actually printed.

Disclosure of tooling is optional; a `Co-authored-by:` trailer is fine if you
want it. Review is applied to the change, not to how it was produced.

## Security

Do not file public issues for vulnerabilities. See [SECURITY.md](SECURITY.md)
and email **shaswot@infervisor.ai**.

## License

Contributions are accepted under the [Apache License 2.0](LICENSE).
