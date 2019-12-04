use clap::{value_t, App, Arg};
use futures::{stream, StreamExt};
use reqwest::Client;
use std::fs::OpenOptions;
use std::fs::{rename, File};
use std::io::{prelude::*, BufReader};
use std::path::Path;
use tokio;

#[tokio::main]
async fn main() -> Result<(), DlmError> {
    let (input_file, max_concurrent_downloads, output_dir) = get_args();
    let file = File::open(input_file)?;
    let reader = BufReader::new(file);
    let client = Client::builder().build()?;
    let output_dir_ref = &output_dir;
    let client_ref = &client;
    stream::iter(reader.lines())
        .for_each_concurrent(max_concurrent_downloads, |link_res| {
            async move {
                match link_res {
                    Err(e) => println!("Error with links iterator {}", e),
                    Ok(link) => {
                        let processed = download_link(link, client_ref, output_dir_ref).await;
                        match processed {
                            Ok(info) => println!("{}", info),
                            Err(e) => println!("Error: {}", e.message),
                        }
                    }
                }
            }
        })
        .await;
    Ok(())
}

async fn download_link(
    link: String,
    client: &Client,
    output_dir: &str,
) -> Result<String, DlmError> {
    let dot_file_extension: String = {
        let tmp: String = link.chars().rev().take_while(|c| c != &'.').collect();
        tmp.chars().rev().collect()
    };
    let file_name: String = {
        let tmp: String = link
            .chars()
            .rev()
            .skip(dot_file_extension.len())
            .take_while(|c| c != &'/')
            .collect();
        tmp.chars().rev().collect()
    };
    let final_name = format!("{}/{}{}", output_dir, file_name, dot_file_extension);
    if Path::new(&final_name).exists() {
        let msg = format!("Skipping {} because it is already present", final_name);
        Ok(msg)
    } else {
        let url_str = link.as_str();
        let head_result = client.head(url_str).send().await?;
        if !head_result.status().is_success() {
            let message = format!("{} {}", url_str, head_result.status());
            Err(DlmError { message })
        } else {
            let tmp_name = format!("{}/{}part", output_dir, file_name);
            let query_range = {
                if Path::new(&tmp_name).exists() {
                    // get existing file size
                    let tmp_size = File::open(&tmp_name)?.metadata()?.len();
                    // get remote file size and range capabilities
                    let content_length = head_result.content_length();
                    let accept_ranges = head_result
                        .headers()
                        .get("Accept-Ranges")
                        .and_then(|ct_len| ct_len.to_str().ok());
                    match (accept_ranges, content_length) {
                        (Some("bytes"), Some(cl)) => {
                            let range_msg = format!("bytes={}-{}", tmp_size, cl);
                            println!("Found part file for {} with size {} and it will be resumed with range {}", tmp_name, tmp_size, range_msg);
                            Some(range_msg)
                        }
                        _ => {
                            println!(
                                "Found part file for {} with size {} but it will be overridden because the server does not support querying a range of bytes",
                                tmp_name, tmp_size
                            );
                            None
                        }
                    }
                } else {
                    println!("Starting to download {}", url_str);
                    None
                }
            };
            // create/open file.part
            let mut file = match query_range {
                Some(_) => OpenOptions::new()
                    .append(true)
                    .create(false)
                    .open(&tmp_name)?,
                None => File::create(&tmp_name)?,
            };
            // building the request
            let mut request = client.get(url_str);
            if let Some(range) = query_range {
                request = request.header("Range", range)
            }
            // initiate file download
            let mut res = request.send().await?;
            // incremental save chunk by chunk into part file
            while let Some(chunk) = res.chunk().await? {
                file.write_all(&chunk)?;
            }
            // rename part file to final
            rename(&tmp_name, &final_name)?;
            let msg = format!("Completed {}", final_name);
            Ok(msg)
        }
    }
}

#[derive(Debug)]
struct DlmError {
    message: String,
}

impl std::convert::From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        DlmError {
            message: e.to_string(),
        }
    }
}

impl std::convert::From<std::io::Error> for DlmError {
    fn from(e: std::io::Error) -> Self {
        DlmError {
            message: e.to_string(),
        }
    }
}

fn get_args() -> (String, usize, String) {
    let matches = App::new("dlm")
        .version("0.1.0")
        .author("Arnaud Gourlay <arnaud.gourlay@gmail.com>")
        .about("Minimal download manager")
        .arg(
            Arg::with_name("maxConcurrentDownloads")
                .help("used to limit //")
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
