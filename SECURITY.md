# Security policy

## Supported versions

Security fixes are made in the latest release of each component (host,
firmware, bridge, display) and ship as a new release of the affected
component. Update through Keyglass Studio, or download the release from
[GitHub Releases](https://github.com/keyglass-dev/keyglass/releases).

## Reporting a vulnerability

Please report vulnerabilities privately, not in public issues or
discussions. Use **Report a vulnerability** on the repository's
[Security tab](https://github.com/keyglass-dev/keyglass/security/advisories/new),
which opens a private advisory visible only to the maintainers.

Include what you can of:

- the component and version affected;
- what an attacker can do, and from where (another program on the same
  computer, a web page, a device on the USB bus, the network);
- steps or a proof of concept to reproduce it.

You can expect an acknowledgement within a week. We keep you informed while
we work on a fix, agree on a disclosure date with you, and credit you in the
advisory unless you prefer otherwise.

## Scope

Of particular interest:

- the daemon's local API, including the third-party app API: access from
  web pages or other users, token handling, consent;
- update verification: index and artifact signatures, key rotation, replay;
- what the host sends to the keyboard and what the keyboard accepts:
  packages, files, firmware images;
- the keyboard's USB interfaces and the bridge's file access.

The keyboard's operating system is not part of Keyglass; problems in it are
outside this project's scope, but we want to hear about anything in it that
affects Keyglass.
