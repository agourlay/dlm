use crate::DlmError;
use crate::DlmError::CliArgumentError;
use crate::user_agents::UserAgent;
use crate::user_agents::UserAgent::{CustomUserAgent, RandomUserAgent};
use clap::{Arg, Command};
use clap::{crate_authors, crate_description, crate_name, crate_version};
use std::path::Path;

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .arg(
            Arg::new("maxConcurrentDownloads")
                .help("Maximum number of concurrent downloads")
                .long("max-concurrent")
                .short('m')
                .num_args(1)
                .value_parser(clap::value_parser!(u32))
                .default_value("2"),
        )
        .arg(
            Arg::new("inputFile")
                .help("Input file with links")
                .long("input-file")
                .short('i')
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("outputDir")
                .help("Output directory for downloads")
                .long("output-dir")
                .short('o')
                .default_value(".")
                .num_args(1),
        )
        .arg(
            Arg::new("userAgent")
                .help("User-Agent header to use")
                .long("user-agent")
                .short('u')
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("randomUserAgent")
                .help("Use a random User-Agent header")
                .long("random-user-agent")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("proxy")
                .help("HTTP proxy to use")
                .long("proxy")
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("retry")
                .help("Number of retries on network error")
                .long("retry")
                .short('r')
                .default_value("10")
                .value_parser(clap::value_parser!(u32))
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("connectionTimeoutSecs")
                .help("Connection timeout in seconds")
                .long("connection-timeout")
                .default_value("10")
                .value_parser(clap::value_parser!(u32))
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("accept")
                .help("Accept header value")
                .long("accept")
                .short('a')
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("acceptInvalidCerts")
                .help("Accept invalid TLS certificates")
                .long("accept-invalid-certs")
                .action(clap::ArgAction::SetTrue),
        )
}

pub struct Arguments {
    pub input_file: String,
    pub max_concurrent_downloads: u32,
    pub output_dir: String,
    pub user_agent: Option<UserAgent>,
    pub proxy: Option<String>,
    pub retry: u32,
    pub connection_timeout_secs: u32,
    pub accept: Option<String>,
    pub accept_invalid_certs: bool,
}

pub fn get_args() -> Result<Arguments, DlmError> {
    let command = command();
    let matches = command.get_matches();

    let max_concurrent_downloads: u32 = *matches
        .get_one("maxConcurrentDownloads")
        .expect("impossible");

    if max_concurrent_downloads == 0 {
        return Err(CliArgumentError {
            message: "'maxConcurrentDownloads' must be positive".to_string(),
        });
    }

    let input_file = matches
        .get_one::<String>("inputFile")
        .expect("impossible")
        .trim()
        .to_string();
    if !Path::new(&input_file).is_file() {
        return Err(CliArgumentError {
            message: "'inputFile' does not exist".to_string(),
        });
    }

    let output_dir = matches
        .get_one::<String>("outputDir")
        .expect("impossible")
        .trim()
        .to_string();
    if !Path::new(&output_dir).is_dir() {
        return Err(CliArgumentError {
            message: "'outputDir' does not exist".to_string(),
        });
    }

    let user_agent: Option<UserAgent> = match matches.get_one::<String>("userAgent") {
        Some(user_agent) => Some(CustomUserAgent(user_agent.clone())),
        None if matches.get_flag("randomUserAgent") => Some(RandomUserAgent),
        _ => None,
    };

    let proxy: Option<String> = matches.get_one::<String>("proxy").cloned();

    // safe match because of default value
    let retry = matches
        .get_one::<u32>("retry")
        .copied()
        .expect("impossible");

    // safe match because of default value
    let connection_timeout_secs = matches
        .get_one::<u32>("connectionTimeoutSecs")
        .copied()
        .expect("impossible");

    let accept = matches.get_one::<String>("accept").cloned();

    let accept_invalid_certs = matches.get_flag("acceptInvalidCerts");

    Ok(Arguments {
        input_file,
        max_concurrent_downloads,
        output_dir,
        user_agent,
        proxy,
        retry,
        connection_timeout_secs,
        accept,
        accept_invalid_certs,
    })
}

#[cfg(test)]
mod args_tests {
    use crate::args::command;

    #[test]
    fn verify_command() {
        command().debug_assert();
    }
}
