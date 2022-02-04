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
dlm x.x.x
Arnaud Gourlay <arnaud.gourlay@gmail.com>
Minimal download manager

USAGE:
    dlm [OPTIONS] --maxConcurrentDownloads <maxConcurrentDownloads> --inputFile <inputFile> --outputDir <outputDir>

OPTIONS:
    -h, --help
            Print help information

    -i, --inputFile <inputFile>
            input file with links

    -M, --maxConcurrentDownloads <maxConcurrentDownloads>
            used to limit the number of downloads in flight

    -o, --outputDir <outputDir>
            output directory for downloads

        --proxy <proxy>
            configure the HTTP client to use a proxy

    -r, --retry <retry>
            configure the number of retries in case of network error [default: 10]

        --randomUserAgent
            sets up a random User-Agent header to be used by the HTTP client

    -U, --userAgent <userAgent>
            User-Agent header to be used by the HTTP client

    -V, --version
            Print version information

```

Example:

```
./dlm --inputFile ~/dlm/links.txt --outputDir ~/dlm/output --maxConcurrentDownloads 2
```