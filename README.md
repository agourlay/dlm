# dlm

[![Build Status](https://travis-ci.org/agourlay/dlm.svg?branch=master)](https://travis-ci.org/agourlay/dlm)

A minimal download manager that works just fine.

## features

- read URLs from a text file (one entry per line)
- control maximum number of concurrent downloads
- resume interrupted downloads if possible
- multi progress bars (made with [indicatif](https://github.com/mitsuhiko/indicatif))

```
./dlm --help
dlm 0.1.0
Arnaud Gourlay <arnaud.gourlay@gmail.com>
Minimal download manager

USAGE:
    dlm --inputFile <inputFile> --maxConcurrentDownloads <maxConcurrentDownloads> --outputDir <outputDir>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -i, --inputFile <inputFile>                              input file with links
    -M, --maxConcurrentDownloads <maxConcurrentDownloads>    used to limit //
    -o, --outputDir <outputDir>                              output directory for downloads
```

## Installation from source

Install [Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html) and then run the following command within the `dlm` directory.

`cargo install --path=.`

Make sure to have `$HOME/.cargo/bin` in your path.