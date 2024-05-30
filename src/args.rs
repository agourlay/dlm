use crate::user_agents::UserAgent;
use crate::user_agents::UserAgent::{CustomUserAgent, RandomUserAgent};
use crate::DlmError;
use crate::DlmError::CliArgumentError;
use clap::{crate_authors, crate_description, crate_name, crate_version};
use clap::{Arg, Command};
use std::path::Path;

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .arg(
            Arg::new("maxConcurrentDownloads")
                .help("used to limit the number of downloads in flight")
                .long("maxConcurrentDownloads")
                .short('M')
                .num_args(1)
                .value_parser(clap::value_parser!(usize))
                .required(true),
        )
        .arg(
            Arg::new("inputFile")
                .help("input file with links")
                .long("inputFile")
                .short('i')
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("outputDir")
                .help("output directory for downloads")
                .long("outputDir")
                .short('o')
                .num_args(1)
                .required(true),
        )
        .arg(
            Arg::new("userAgent")
                .help("User-Agent header to be used by the HTTP client")
                .long("userAgent")
                .short('U')
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("randomUserAgent")
                .help("sets up a random User-Agent header to be used by the HTTP client")
                .long("randomUserAgent")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("proxy")
                .help("configure the HTTP client to use a proxy")
                .long("proxy")
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("retry")
                .help("configure the number of retries in case of network error")
                .long("retry")
                .short('r')
                .default_value("10")
                .value_parser(clap::value_parser!(usize))
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("connectionTimeoutSecs")
                .help("configure connection timeout in seconds for the HTTP client")
                .long("connectionTimeoutSecs")
                .default_value("10")
                .value_parser(clap::value_parser!(usize))
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("accept")
                .help("Accept header to be used by the HTTP client request")
                .long("accept")
                .short('A')
                .num_args(1)
                .required(false),
        )
}

pub struct Arguments {
    pub input_file: String,
    pub max_concurrent_downloads: usize,
    pub output_dir: String,
    pub user_agent: Option<UserAgent>,
    pub proxy: Option<String>,
    pub retry: usize,
    pub connection_timeout_secs: usize,
    pub accept: Option<String>,
}

pub fn get_args() -> Result<Arguments, DlmError> {
    let command = command();
    let matches = command.get_matches();

    let max_concurrent_downloads: usize = *matches
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
        Some(user_agent) => Some(CustomUserAgent(user_agent.to_string())),
        None if matches.get_flag("randomUserAgent") => Some(RandomUserAgent),
        _ => None,
    };

    let proxy: Option<String> = matches.get_one::<String>("proxy").cloned();

    // safe match because of default value
    let retry = matches
        .get_one::<usize>("retry")
        .copied()
        .expect("impossible");

    // safe match because of default value
    let connection_timeout_secs = matches
        .get_one::<usize>("connectionTimeoutSecs")
        .copied()
        .expect("impossible");

    let accept = matches.get_one::<String>("accept").cloned();

    Ok(Arguments {
        input_file,
        max_concurrent_downloads,
        output_dir,
        user_agent,
        proxy,
        retry,
        connection_timeout_secs,
        accept,
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
