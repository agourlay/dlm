# dlm

[![Build Status](https://travis-ci.org/agourlay/dlm.svg?branch=master)](https://travis-ci.org/agourlay/dlm)

A minimal download manager accepting as input a file with one link per line.

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
