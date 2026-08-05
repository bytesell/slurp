# Security Policy

## Reporting a vulnerability

Report security issues privately. Do not open a public GitHub issue for one.

**Use GitHub's private vulnerability reporting.** Go to the repository's **Security** tab and press **Report a vulnerability**. That opens a private draft advisory visible only to you and the maintainers, and it lets us collaborate on a fix and issue a CVE before anything is public.

That is the only reporting channel. There is deliberately no security email address: an unmonitored inbox listed in a `SECURITY.md` looks like a channel and silently is not. If GitHub advisories are unavailable to you, open a public issue saying only that you have a security report and asking a maintainer to make contact, with no details of the vulnerability in it.

Please include:

- A description of the vulnerability and the affected component
- Steps to reproduce, or a minimal proof of concept
- What an attacker could achieve
- Any mitigation you would suggest

We aim to acknowledge a report within 72 hours and to ship a fix for a critical issue within 30 days. Slurp is a small pre-1.0 project, so treat those as targets rather than a guarantee, and please chase us if a report goes quiet.

## Supported versions

Only the latest release is supported. There are no maintenance branches: fixes land on `main` and go out in the next release.

| Version | Supported |
|---|---|
| 0.1.x | yes |
| anything older | no |

## What is in scope

Before reporting, check [docs/security-model.md](./docs/security-model.md). It is the threat model: the trust levels, which escaper slurp applies in each interpolation context, the compile-time gates, the SSRF and resource limits in the image service, the dev server's guards, and the limitations that are known and deliberate. A report against a documented trust assumption (`{html}` without `sanitize`, an attribute spread carrying untrusted data, a host that seeds a secret into the render context) is expected behaviour rather than a vulnerability.
