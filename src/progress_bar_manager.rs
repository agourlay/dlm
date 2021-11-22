use chrono::Local;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::cmp::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

const PENDING: &str = "pending";

// TODO use Tokio's mpsc to not mix Tasks with Threads
pub struct ProgressBarManager {
    main_pb: ProgressBar,
    file_pb_count: usize,
    tx: Sender<ProgressBar>,
    rx: Receiver<ProgressBar>
}

impl ProgressBarManager {

    pub fn get_main_pb_ref(&self) -> &ProgressBar {
        &self.main_pb
    }

    pub fn get_rx_ref(&self) -> &Receiver<ProgressBar> {
        &self.rx
    }

    pub fn get_tx_ref(&self) -> &Sender<ProgressBar> {
        &self.tx
    }

    pub fn init(max_concurrent_downloads: usize, main_pb_len: u64) -> Self {
        let mp = MultiProgress::new();

        // main progress bar
        let main_style = ProgressStyle::default_bar().template("{bar:130} {pos}/{len}");
        let main_pb = mp.add(ProgressBar::new(0));
        main_pb.set_style(main_style);
        main_pb.set_length(main_pb_len);

        let (tx, rx): (Sender<ProgressBar>, Receiver<ProgressBar>) = mpsc::channel();

        let dl_style = ProgressStyle::default_bar()
            .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
            .progress_chars("#>-");

        // `max_concurrent_downloads` progress bars are shared between the threads at anytime
        for _ in 0..max_concurrent_downloads {
            let file_pb = mp.add(ProgressBar::new(0));
            file_pb.set_style(dl_style.clone());
            file_pb.set_message(ProgressBarManager::message_progress_bar(PENDING));
            tx.send(file_pb).expect("channel should not fail");
        }

        // Render MultiProgress bar
        let _ = thread::spawn(move || {
            mp.join_and_clear().unwrap();
        });

        ProgressBarManager {
            main_pb,
            file_pb_count: max_concurrent_downloads,
            rx,
            tx
        }
    }

    pub fn finish_all(&self) {
        for _ in 0..self.file_pb_count {
            let pb = self.rx.recv().expect("claiming channel should not fail");
            pb.finish();
        }
        self.main_pb.finish();
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

    pub fn logger(pb: &ProgressBar, msg: String) {
        pb.println(format!("[{}] {}", Local::now().naive_local(), msg));
    }
}
