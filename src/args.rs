use crate::user_agents::UserAgent;
use crate::user_agents::UserAgent::{CustomUserAgent, RandomUserAgent};
use crate::DlmError;
use crate::DlmError::CliArgumentError;
use clap::{App, Arg};
use std::path::Path;

fn app() -> clap::App<'static> {
    App::new("dlm")
        .version("0.2.2")
        .author("Arnaud Gourlay <arnaud.gourlay@gmail.com>")
        .about("Minimal download manager")
        .arg(
            Arg::new("maxConcurrentDownloads")
                .help("used to limit the number of downloads in flight")
                .long("maxConcurrentDownloads")
                .short('M')
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("inputFile")
                .help("input file with links")
                .long("inputFile")
                .short('i')
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("outputDir")
                .help("output directory for downloads")
                .long("outputDir")
                .short('o')
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::new("userAgent")
                .help("User-Agent header to be used by the HTTP client")
                .long("userAgent")
                .short('U')
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::new("randomUserAgent")
                .help("sets up a random User-Agent header to be used by the HTTP client")
                .long("randomUserAgent")
                .required(false),
        )
}

pub fn get_args() -> Result<(String, usize, String, Option<UserAgent>), DlmError> {
    let app = app();
    let matches = app.get_matches();

    let max_concurrent_downloads =
        matches
            .value_of_t("maxConcurrentDownloads")
            .map_err(|_| CliArgumentError {
                message: "maxConcurrentDownloads was not an integer".to_string(),
            })?;

    if max_concurrent_downloads == 0 {
        return Err(CliArgumentError {
            message: "maxConcurrentDownloads was not an integer".to_string(),
        });
    }

    let input_file = matches.value_of("inputFile").expect("impossible");
    if !Path::new(input_file).is_file() {
        return Err(CliArgumentError {
            message: "inputFile does not exist".to_string(),
        });
    }

    let output_dir = matches.value_of("outputDir").expect("impossible");
    if !Path::new(output_dir).is_dir() {
        return Err(CliArgumentError {
            message: "outputDir does not exist".to_string(),
        });
    }

    let user_agent: Option<UserAgent> = if matches.is_present("userAgent") {
        Some(CustomUserAgent(
            matches
                .value_of("userAgent")
                .expect("impossible")
                .to_string(),
        ))
    } else if matches.is_present("randomUserAgent") {
        Some(RandomUserAgent)
    } else {
        None
    };

    Ok((
        input_file.to_string(),
        max_concurrent_downloads,
        output_dir.to_string(),
        user_agent,
    ))
}

#[cfg(test)]
mod args_tests {
    use crate::args::app;

    #[test]
    fn verify_app() {
        app().debug_assert();
    }
}
