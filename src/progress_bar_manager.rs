use chrono::Local;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::cmp::Ordering;
use async_channel::{Receiver, Sender};
use tokio::task::JoinHandle;
use crate::DlmError;

const PENDING: &str = "pending";

pub struct ProgressBarManager {
    main_pb: ProgressBar,
    file_pb_count: usize,
    pub tx: Sender<ProgressBar>,
    pub rx: Receiver<ProgressBar>
}

impl ProgressBarManager {

    pub async fn init(max_concurrent_downloads: usize, main_pb_len: u64) -> (JoinHandle<()>, ProgressBarManager) {
        let mp = MultiProgress::new();

        // main progress bar
        let main_style = ProgressStyle::default_bar().template("{bar:133} {pos}/{len}");
        let main_pb = mp.add(ProgressBar::new(0));
        main_pb.set_style(main_style);
        main_pb.set_length(main_pb_len);

        // If you need a multi-producer multi-consumer channel where only one consumer sees each message, you can use the async-channel crate.
        // There are also channels for use outside of asynchronous Rust, such as std::sync::mpsc and crossbeam::channel.
        // These channels wait for messages by blocking the thread, which is not allowed in asynchronous code.
        // ref: https://tokio.rs/tokio/tutorial/channels
        let (tx, rx): (Sender<ProgressBar>, Receiver<ProgressBar>) = async_channel::bounded(max_concurrent_downloads);

        let dl_style = ProgressStyle::default_bar()
            .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
            .progress_chars("#>-");

        // `max_concurrent_downloads` progress bars are shared between the threads at anytime
        for _ in 0..max_concurrent_downloads {
            let file_pb = mp.add(ProgressBar::new(0));
            file_pb.set_style(dl_style.clone());
            file_pb.set_message(ProgressBarManager::message_progress_bar(PENDING));
            tx.send(file_pb).await.expect("channel should not fail");
        }

        // Render MultiProgress bar async. in a dedicated blocking thread
        let h = tokio::task::spawn_blocking(move || {
            mp.join_and_clear().unwrap();
        });

        let pbm = ProgressBarManager {
            main_pb,
            file_pb_count: max_concurrent_downloads,
            rx,
            tx
        };
        (h, pbm)
    }

    pub async fn finish_all(&self) -> Result<(), DlmError> {
        for _ in 0..self.file_pb_count {
            let pb = self.rx.recv().await?;
            pb.finish();
        }
        self.main_pb.finish();
        Ok(())
    }

    pub fn increment_global_progress(&self) {
        self.main_pb.inc(1)
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

    pub fn log_above_progress_bars(&self, msg: String) {
        ProgressBarManager::log_above_progress_bar(&self.main_pb, msg)
    }

    pub fn log_above_progress_bar(pb: &ProgressBar, msg: String) {
        pb.println(format!("[{}] {}", Local::now().naive_local(), msg));
    }
}
