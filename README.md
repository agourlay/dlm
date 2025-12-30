# dlm

[![Build status](https://github.com/agourlay/dlm/actions/workflows/ci.yml/badge.svg)](https://github.com/agourlay/dlm/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/dlm.svg)](https://crates.io/crates/dlm)

A minimal HTTP download manager that works just fine.

## Features

- read URLs from a text file (one entry per line)
- control maximum number of concurrent downloads
- resume interrupted downloads if possible (using HTTP range)
- automatically retry re-establishing download in case of timeout or hanging connection
- multi progress bars (made with [indicatif](https://github.com/mitsuhiko/indicatif))
- native support for proxies and redirects

```
./dlm --help
Minimal download manager

Usage: dlm [OPTIONS] --input-file <inputFile>

Options:
  -m, --max-concurrent <maxConcurrentDownloads>
          Maximum number of concurrent downloads [default: 2]
  -i, --input-file <inputFile>
          Input file with links
  -o, --output-dir <outputDir>
          Output directory for downloads [default: .]
  -u, --user-agent <userAgent>
          User-Agent header to use
      --random-user-agent
          Use a random User-Agent header
      --proxy <proxy>
          HTTP proxy to use
  -r, --retry <retry>
          Number of retries on network error [default: 10]
      --connection-timeout <connectionTimeoutSecs>
          Connection timeout in seconds [default: 10]
  -a, --accept <accept>
          Accept header value
      --accept-invalid-certs
          Accept invalid TLS certificates
  -h, --help
          Print help
  -V, --version
          Print version
```

## Examples

- Quick run in current directory:

```bash
./dlm --input-file ~/dlm/links.txt
```

- With output directory and max concurrent download control

```bash
./dlm --input-file ~/dlm/links.txt --output-dir ~/dlm/output --max-concurrent 2
```

## Installation

### Releases

Using the provided binaries in https://github.com/agourlay/dlm/releases

### Crates.io

Using Cargo via [crates.io](https://crates.io/crates/dlm).

```bash
cargo install dlm
```
