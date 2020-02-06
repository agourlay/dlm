use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::cmp::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

const PENDING: &str = "pending";

pub fn init_progress_bars(
    max_concurrent_downloads: usize,
) -> (Sender<ProgressBar>, Receiver<ProgressBar>) {
    let mp = MultiProgress::new();
    let style = ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .progress_chars("#>-");

    let (tx, rx): (Sender<ProgressBar>, Receiver<ProgressBar>) = mpsc::channel();

    // `max_concurrent_downloads` progress bars are shared between the threads at anytime
    for _ in 0..max_concurrent_downloads {
        let pb = mp.add(ProgressBar::new(0));
        pb.set_style(style.clone());
        pb.set_message(message_progress_bar(PENDING).as_str());
        tx.send(pb).expect("channel should not fail");
    }

    // Render MultiProgress bar
    let _ = thread::spawn(move || {
        mp.join_and_clear().unwrap();
    });

    (tx, rx)
}

pub fn message_progress_bar(s: &str) -> String {
    let max = 35; // arbitrary limit
    let count = s.chars().count();

    match count.cmp(&max) {
        Ordering::Greater => s.chars().take(max).collect(),
        Ordering::Equal => s.to_string(),
        Ordering::Less => format!("{}{}", s, " ".repeat(max - count)),
    }
}
