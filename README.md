# dlm

[![Build status](https://github.com/agourlay/dlm/actions/workflows/ci.yml/badge.svg)](https://github.com/agourlay/dlm/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/dlm.svg)](https://crates.io/crates/dlm)

A minimal HTTP download manager that works just fine.

## Features

- read URLs from a text file
- control maximum number of concurrent downloads
- resume interrupted downloads if possible (using HTTP range)
- automatically retry re-establishing download in case of timeout or hanging connection
- multi progress bars (made with [indicatif](https://github.com/mitsuhiko/indicatif))
- native support for proxies and redirects

### Input file format

- one URL per line
- empty lines are ignored
- lines starting with `#` are ignored as comment

## Usage

```
./dlm --help
Minimal download manager

Usage: dlm [OPTIONS] [URL]

Arguments:
  [URL]  Direct URL to download

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
      --list-user-agents
          Print the built-in User-Agent pool and exit
      --proxy <proxy>
          HTTP proxy to use
  -r, --retry <retry>
          Number of retries on network error [default: 10]
      --connection-timeout <connectionTimeoutSecs>
          Connection timeout in seconds [default: 10]
      --read-timeout <readTimeoutSecs>
          Read timeout in seconds (0 = wait indefinitely) [default: 60]
  -k, --insecure
          Accept invalid TLS certificates
  -H, --header <header>
          Custom request header (repeatable, format 'Name: Value')
      --user <user>
          Basic auth credentials in format 'user:password'
  -h, --help
          Print help
  -V, --version
          Print version
```

## Examples

- Download single file

```bash
./dlm https://storage.com/my-file.zip
```

- Download several files into current directory

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
