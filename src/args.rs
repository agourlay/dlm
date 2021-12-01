use clap::{value_t, App, Arg};
use std::path::Path;

pub fn get_args() -> (String, usize, String) {
    let matches = App::new("dlm")
        .version("0.2.0")
        .author("Arnaud Gourlay <arnaud.gourlay@gmail.com>")
        .about("Minimal download manager")
        .arg(
            Arg::with_name("maxConcurrentDownloads")
                .help("used to limit the number of downloads in flight")
                .long("maxConcurrentDownloads")
                .short("M")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("inputFile")
                .help("input file with links")
                .long("inputFile")
                .short("i")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("outputDir")
                .help("output directory for downloads")
                .long("outputDir")
                .short("o")
                .takes_value(true)
                .required(true),
        )
        .get_matches();

    let max_concurrent_downloads = value_t!(matches, "maxConcurrentDownloads", usize)
        .expect("maxConcurrentDownloads was not an integer");
    if max_concurrent_downloads == 0 {
        panic!("invalid maxConcurrentDownloads - must be a positive integer")
    }

    let input_file = matches.value_of("inputFile").expect("impossible");
    if !Path::new(input_file).is_file() {
        panic!("inputFile does not exist")
    }

    let output_dir = matches.value_of("outputDir").expect("impossible");
    if !Path::new(output_dir).is_dir() {
        panic!("outputDir does not exist")
    }

    (
        input_file.to_string(),
        max_concurrent_downloads,
        output_dir.to_string(),
    )
}
