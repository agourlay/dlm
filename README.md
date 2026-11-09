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
- end of run report with per outcome counts, transferred volume and average speed

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
      --no-color
          Disable coloured output (also honours the NO_COLOR env var)
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

## Colours

Warnings and errors are coloured on a terminal. Colours are dropped
automatically when the output is redirected or when `NO_COLOR` is set, and
`--no-color` turns them off outright for terminals that report more support
than they actually have.

## Skipping and resuming

`dlm` considers a link already downloaded when a file of that name sits in the
output directory - that is the whole test. There is no size comparison, no
checksum and no timestamp check, so a truncated or unrelated file left by
another tool counts as done; delete it to force a fresh download.

The name comes from the link itself whenever one carrying an extension can be
derived from it - normally the last path segment - and in that case the file is
skipped without contacting the server at all. Otherwise `dlm` sends a `HEAD`
request first and takes the name from the `Content-Disposition` header, or from
the target of a redirect.

A link ending in a server-side script (`.php`, `.asp`, `.aspx`, `.jsp`, `.cgi`,
`.pl`, `.cfm`) names the page producing the download rather than the file, so it
is treated like a link without an extension. When the server gives no name, the
query is kept in it: `get.php?id=1` and `get.php?id=2` land in different files.

Links ending in the same file name therefore map to the same file on disk, even
when they point at different content. Reached one after another, the first
downloads and the rest are reported as already completed. Reached at the same
time, they collide on the same `.part` file and all but one of them fail.

An unfinished download sits next to it as `<name>.part`, and what happens to it
on the next run depends on what the server reports:

| `.part` compared to the server's size | outcome |
| --- | --- |
| the same, and the server serves ranges | finalized as is, nothing is fetched |
| smaller, and the server serves ranges | resumed from that offset |
| larger | discarded, downloaded again from scratch |
| any size, when the server serves no ranges or reports no size | discarded, downloaded again from scratch |

A resume only stands if the server answers `206 Partial Content`. One that
advertises `Accept-Ranges` but replies `200` with the whole file has the `.part`
replaced rather than appended to.

## End of run report

Every run ends with a breakdown of what actually happened:

```
Finished in 12m 04s - 53 completed, 2 skipped, 1 failed
Downloaded 4.20GiB at an average speed of 5.94MiB/s
```

- `completed`: files downloaded during this run
- `skipped`: files already present in the output directory
- `failed`: links that could not be downloaded, retries included

The volume and the speed describe the network traffic of the run: bytes already
present in a resumed `.part` file are not counted, and the average speed is wall
clock based, so it also covers the time spent waiting on servers and on retries.

The report is printed on `stdout` and is kept when the output is redirected to a
file or piped into another command, unlike the progress bars.

A run stopped with `ctrl-c` is reported too, so an interrupted run still says
what it got through:

```
Interrupted after 6s - 12 completed, 1 skipped, 0 failed
Downloaded 3.00MiB at an average speed of 512.00KiB/s
```

The downloads still in flight count as neither completed nor failed: their
`.part` files are left in place for the next run to resume.

## Exit code

`dlm` exits with `0` only if no download failed, making it usable in scripts:

```bash
./dlm --input-file ~/dlm/links.txt || echo "some downloads failed"
```

A run interrupted with `ctrl-c` also exits with a non-zero code.

## Installation

### Releases

Using the provided binaries in https://github.com/agourlay/dlm/releases

### Crates.io

Using Cargo via [crates.io](https://crates.io/crates/dlm).

```bash
cargo install dlm
```
