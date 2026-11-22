# Security policy

## Reporting a vulnerability

Report privately through GitHub Security Advisories:
<https://github.com/danyghraizi/convertme/security/advisories/new>

Please do not open a public issue for anything that could be exploited. Include the
version (`convertme --version`), the platform, and the smallest input that shows the
problem. A file that triggers a crash is usually enough; you do not need to write an
exploit.

Expect an initial response within a week. There is one maintainer, so a fix may take
longer than the response; you will be told which it is.

## What the threat model covers

ConvertMe is an offline desktop application. It reads files you point it at, runs
external converter programs over argv arrays, and writes a new file next to the
source. In scope:

- **Hostile input files**, whether they reach our Rust parsers or a bundled tool.
- **Path handling**: a source path must never be overwritten, and a crafted filename
  must not become a shell word, an argument injection, or a path outside the job's
  work directory.
- **The Tauri shell**: unexpected IPC surface, `asset` protocol scope escapes, or a
  webview navigation that should not be possible.
- **Update check**: the one network request the app can make. A spoofed response
  must not be able to run code or replace an installed binary.

Out of scope: a vulnerability in a converter you installed yourself and that
ConvertMe only invokes (report it upstream, and to us if our invocation makes it
worse), and anything that requires an attacker who already has your user account.

## What we do not do

No telemetry, no file contents leave the machine, no crash reports are uploaded.
Bundled third-party tools and their licences are listed in `NOTICE`.
