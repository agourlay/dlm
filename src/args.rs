use crate::DlmError;
use crate::DlmError::CliArgumentError;
use crate::user_agents::UserAgent;
use crate::user_agents::UserAgent::{CustomUserAgent, RandomUserAgent};
use crate::user_agents::print_user_agents;
use clap::{Arg, Command};
use clap::{crate_authors, crate_description, crate_name, crate_version};
use std::path::{Path, PathBuf};

fn command() -> Command {
    Command::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!("\n"))
        .about(crate_description!())
        .arg(
            Arg::new("url")
                .help("Direct URL to download")
                .value_name("URL")
                .index(1)
                .num_args(1)
                .required(false)
                .conflicts_with("inputFile"),
        )
        .arg(
            Arg::new("maxConcurrentDownloads")
                .help("Maximum number of concurrent downloads")
                .long("max-concurrent")
                .short('m')
                .num_args(1)
                // reject 0: no progress bar would ever be available to claim,
                // so every download would block forever.
                .value_parser(clap::value_parser!(u32).range(1..))
                .default_value("2"),
        )
        .arg(
            Arg::new("inputFile")
                .help("Input file with links")
                .long("input-file")
                .short('i')
                .num_args(1)
                .conflicts_with("url"),
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
            Arg::new("listUserAgents")
                .help("Print the built-in User-Agent pool and exit")
                .long("list-user-agents")
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
                // reject 0: a zero Duration makes reqwest's connect/read
                // timeouts fire immediately, breaking every download.
                .value_parser(clap::value_parser!(u32).range(1..))
                .num_args(1)
                .required(false),
        )
        .arg(
            Arg::new("insecure")
                .help("Accept invalid TLS certificates")
                .long("insecure")
                .short('k')
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("header")
                .help("Custom request header (repeatable, format 'Name: Value')")
                .long("header")
                .short('H')
                .num_args(1)
                .action(clap::ArgAction::Append)
                .required(false),
        )
        .arg(
            Arg::new("user")
                .help("Basic auth credentials in format 'user:password'")
                .long("user")
                .num_args(1)
                .required(false),
        )
}

pub enum Input {
    File(String),
    Url(String),
}

pub struct Arguments {
    pub input: Input,
    pub max_concurrent_downloads: u32,
    pub output_dir: PathBuf,
    pub user_agent: Option<UserAgent>,
    pub proxy: Option<String>,
    pub retry: u32,
    pub connection_timeout_secs: u32,
    pub insecure: bool,
    pub headers: Vec<(String, String)>,
    pub basic_auth: Option<(String, String)>,
}

/// Parse a single `Name: Value` header argument.
fn parse_header(raw: &str) -> Result<(String, String), DlmError> {
    let (name, value) = raw.split_once(':').ok_or_else(|| CliArgumentError {
        message: format!("invalid header '{raw}', expected 'Name: Value'"),
    })?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() {
        return Err(CliArgumentError {
            message: format!("invalid header '{raw}', name cannot be empty"),
        });
    }
    Ok((name.to_string(), value.to_string()))
}

/// Parse a `user:password` basic-auth argument. The password may contain colons.
fn parse_basic_auth(raw: &str) -> Result<(String, String), DlmError> {
    let (user, pass) = raw.split_once(':').ok_or_else(|| CliArgumentError {
        message: "invalid '--user', expected 'user:password'".to_string(),
    })?;
    if user.is_empty() {
        return Err(CliArgumentError {
            message: "invalid '--user', user cannot be empty".to_string(),
        });
    }
    Ok((user.to_string(), pass.to_string()))
}

pub fn get_args() -> Result<Arguments, DlmError> {
    let command = command();
    let matches = command.get_matches();

    // Print-and-exit flags handled before any URL/input validation,
    // so `dlm --list-user-agents` works without other arguments.
    if matches.get_flag("listUserAgents") {
        print_user_agents();
        std::process::exit(0);
    }

    let max_concurrent_downloads: u32 = *matches
        .get_one("maxConcurrentDownloads")
        .expect("impossible");

    let url = matches.get_one::<String>("url");
    let input_file = matches.get_one::<String>("inputFile");

    // Process mutually exclusive inputs
    let input = match (url, input_file) {
        (None, None) | (Some(_), Some(_)) => Err(CliArgumentError {
            message: "provide either a URL or --input-file".to_string(),
        }),
        (Some(url), None) => Ok(Input::Url(url.trim().to_string())),
        (None, Some(file)) => {
            let input_file = file.trim();
            if Path::new(input_file).is_file() {
                Ok(Input::File(input_file.to_string()))
            } else {
                Err(CliArgumentError {
                    message: "'inputFile' does not exist".to_string(),
                })
            }
        }
    };
    let input = input?;

    let output_dir = PathBuf::from(
        matches
            .get_one::<String>("outputDir")
            .expect("impossible")
            .trim(),
    );
    if !output_dir.is_dir() {
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

    let insecure = matches.get_flag("insecure");

    let headers = matches
        .get_many::<String>("header")
        .into_iter()
        .flatten()
        .map(|s| parse_header(s))
        .collect::<Result<Vec<_>, _>>()?;

    let basic_auth = matches
        .get_one::<String>("user")
        .map(|s| parse_basic_auth(s))
        .transpose()?;

    Ok(Arguments {
        input,
        max_concurrent_downloads,
        output_dir,
        user_agent,
        proxy,
        retry,
        connection_timeout_secs,
        insecure,
        headers,
        basic_auth,
    })
}

#[cfg(test)]
mod args_tests {
    use crate::args::command;

    #[test]
    fn verify_assert_command() {
        command().debug_assert();
    }

    #[test]
    fn verify_help_command() {
        let help = command().term_width(80).render_long_help();
        let help_str = format!("{help}");
        let normalize = |s: &str| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let actual = normalize(&help_str);

        let expected = normalize(
            r"
    Minimal download manager
    Usage: dlm [OPTIONS] [URL]
    Arguments:
    [URL]
    Direct URL to download
    Options:
    -m, --max-concurrent <maxConcurrentDownloads>
    Maximum number of concurrent downloads
    [default: 2]
    -i, --input-file <inputFile>
    Input file with links
    -o, --output-dir <outputDir>
    Output directory for downloads
    [default: .]
    -u, --user-agent <userAgent>
    User-Agent header to use
    --random-user-agent
    Use a random User-Agent header
    --list-user-agents
    Print the built-in User-Agent pool and exit
    --proxy <proxy>
    HTTP proxy to use
    -r, --retry <retry>
    Number of retries on network error
    [default: 10]
    --connection-timeout <connectionTimeoutSecs>
    Connection timeout in seconds
    [default: 10]
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
    ",
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn parse_header_ok() {
        let (n, v) = super::parse_header("Authorization: Bearer xyz").unwrap();
        assert_eq!(n, "Authorization");
        assert_eq!(v, "Bearer xyz");
    }

    #[test]
    fn parse_header_strips_whitespace() {
        let (n, v) = super::parse_header("  X-Trace-Id  :  abc-123  ").unwrap();
        assert_eq!(n, "X-Trace-Id");
        assert_eq!(v, "abc-123");
    }

    #[test]
    fn parse_header_value_with_colons() {
        // Only the first colon is the delimiter — header values may contain colons.
        let (n, v) = super::parse_header("X-Range: bytes=0-100:200").unwrap();
        assert_eq!(n, "X-Range");
        assert_eq!(v, "bytes=0-100:200");
    }

    #[test]
    fn parse_header_no_colon_errors() {
        assert!(super::parse_header("nocolon").is_err());
    }

    #[test]
    fn parse_header_empty_name_errors() {
        assert!(super::parse_header(": value").is_err());
    }

    #[test]
    fn parse_basic_auth_ok() {
        let (u, p) = super::parse_basic_auth("alice:s3cret").unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "s3cret");
    }

    #[test]
    fn parse_basic_auth_password_with_colons() {
        let (u, p) = super::parse_basic_auth("alice:foo:bar").unwrap();
        assert_eq!(u, "alice");
        assert_eq!(p, "foo:bar");
    }

    #[test]
    fn parse_basic_auth_empty_user_errors() {
        assert!(super::parse_basic_auth(":pass").is_err());
    }

    #[test]
    fn parse_basic_auth_no_colon_errors() {
        assert!(super::parse_basic_auth("alice").is_err());
    }
}
