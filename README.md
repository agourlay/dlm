# dlm
[![Build](https://github.com/agourlay/dlm/actions/workflows/ci.yml/badge.svg)](https://github.com/agourlay/dlm/actions/workflows/ci.yml)

A minimal HTTP download manager that works just fine.

## features

- read URLs from a text file (one entry per line)
- control maximum number of concurrent downloads
- resume interrupted downloads if possible (using HTTP range)
- automatically retry re-establishing download in case of timeout or hanging connection
- multi progress bars (made with [indicatif](https://github.com/mitsuhiko/indicatif))
- native support for proxies and redirects

```
./dlm --help
Minimal download manager

Usage: dlm [OPTIONS] --maxConcurrentDownloads <maxConcurrentDownloads> --inputFile <inputFile> --outputDir <outputDir>

Options:
  -M, --maxConcurrentDownloads <maxConcurrentDownloads>
          used to limit the number of downloads in flight
  -i, --inputFile <inputFile>
          input file with links
  -o, --outputDir <outputDir>
          output directory for downloads
  -U, --userAgent <userAgent>
          User-Agent header to be used by the HTTP client
      --randomUserAgent
          sets up a random User-Agent header to be used by the HTTP client
      --proxy <proxy>
          configure the HTTP client to use a proxy
  -r, --retry <retry>
          configure the number of retries in case of network error [default: 10]
      --connectionTimeoutSecs <connectionTimeoutSecs>
          configure connection timeout in seconds for the HTTP client [default: 10]
  -h, --help
          Print help
  -V, --version
          Print version

```

Example:

```
./dlm --inputFile ~/dlm/links.txt --outputDir ~/dlm/output --maxConcurrentDownloads 2
```