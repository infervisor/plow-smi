# Security Policy

## Supported Versions

Security fixes target the default branch of this repository. If you are running
an older commit, please upgrade before reporting version-specific issues.

## Reporting a Vulnerability

**Do not** open a public GitHub issue for security vulnerabilities.

Please report vulnerabilities privately by email to:

**shaswot@infervisor.ai**

Include as much of the following as you can:

- A clear description of the issue and its impact
- Steps to reproduce, or a proof of concept
- Affected components (`plows-gpu`, `plows-exporter`, `plows-top`, `plows-ctl`,
  `plow-smi`, deps, etc.)
- Your assessment of severity, if known

We aim to acknowledge reports within a few business days. After triage we will
coordinate disclosure timing with you. Please give us a reasonable window to
investigate and ship a fix before any public discussion.

## Scope

In scope: the GPU discovery/metrics layer, the Prometheus exporter's HTTP
surface, the TUI, the power/clock control paths, and build/packaging (Cargo,
Nix) shipped in this repository.

Out of scope (unless Plow SMI is clearly at fault): third-party GPU vendor
drivers/libraries (`libnvidia-ml`, the `amdgpu` kernel driver, `libze_loader`), or issues
that require physical or privileged host access beyond normal deployment.
