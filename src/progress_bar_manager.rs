use crate::DlmError;
use async_channel::{Receiver, Sender};
use chrono::Local;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::cmp::{Ordering, min};

const PENDING: &str = "pending";

pub struct ProgressBarManager {
    main_pb: ProgressBar,
    file_pb_count: u64,
    pub tx: Sender<ProgressBar>,
    pub rx: Receiver<ProgressBar>,
}

impl ProgressBarManager {
    pub async fn init(max_concurrent_downloads: u32, main_pb_len: u64) -> Self {
        let mp = MultiProgress::new();
        // Refresh terminal 5 times per seconds
        let draw_target = ProgressDrawTarget::stdout_with_hz(5);
        mp.set_draw_target(draw_target);

        // main progress bar
        let main_style = ProgressStyle::default_bar()
            .template("{bar:133} {pos}/{len}")
            .expect("templating should not fail");
        let main_pb = mp.add(ProgressBar::new(0));
        main_pb.set_style(main_style);
        main_pb.set_length(main_pb_len);

        // `file_pb_count` progress bars are shared between the threads at anytime
        let file_pb_count = min(max_concurrent_downloads as u64, main_pb_len);

        // If you need a multi-producer multi-consumer channel where only one consumer sees each message, you can use the async-channel crate.
        // There are also channels for use outside of asynchronous Rust, such as std::sync::mpsc and crossbeam::channel.
        // These channels wait for messages by blocking the thread, which is not allowed in asynchronous code.
        // ref: https://tokio.rs/tokio/tutorial/channels
        let (tx, rx): (Sender<ProgressBar>, Receiver<ProgressBar>) =
            async_channel::bounded(file_pb_count as usize);

        let dl_style = ProgressStyle::default_bar()
            .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
            .expect("templating should not fail")
            .progress_chars("#>-");

        for _ in 0..file_pb_count {
            let file_pb = mp.add(ProgressBar::new(0));
            file_pb.set_style(dl_style.clone());
            file_pb.set_message(Self::message_progress_bar(PENDING));
            tx.send(file_pb).await.expect("channel should not fail");
        }

        Self {
            main_pb,
            file_pb_count,
            tx,
            rx,
        }
    }

    pub async fn finish_all(&self) -> Result<(), DlmError> {
        for _ in 0..self.file_pb_count {
            let pb = self.rx.recv().await?;
            pb.finish_and_clear();
        }
        self.main_pb.finish();
        Ok(())
    }

    pub fn increment_global_progress(&self) {
        self.main_pb.inc(1);
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

    pub fn log_above_progress_bars(&self, msg: &str) {
        Self::log_above_progress_bar(&self.main_pb, msg);
    }

    fn log_above_progress_bar(pb: &ProgressBar, msg: &str) {
        pb.println(format!(
            "[{}] {}",
            Local::now().naive_local().format("%Y-%m-%d %H:%M:%S"),
            msg
        ));
    }

    pub fn reset_progress_bar(pb: &ProgressBar) {
        pb.reset();
        pb.set_message(Self::message_progress_bar(PENDING));
    }
}
