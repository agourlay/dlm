use crate::DlmError;
use async_channel::{Receiver, Sender};
use console::style;
use indicatif::{
    HumanBytes, HumanDuration, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState,
    ProgressStyle,
};
use jiff::Zoned;
use std::cmp::{Ordering, min};

const PENDING: &str = "pending";

/// Width assumed when stdout is not a terminal, matching what `console` falls
/// back to. The bars draw nowhere in that case, so it only picks the layout.
const FALLBACK_TERM_WIDTH: usize = 80;

/// Terminals at least this wide can carry the full download line - elapsed
/// time included - and still leave a usable bar.
const ROOMY_TERM_WIDTH: usize = 120;

/// Bounds on the filename column. Wide enough to recognise a file, never so
/// wide that it crowds out the bar on a small terminal.
const MIN_MSG_WIDTH: usize = 12;
const MAX_MSG_WIDTH: usize = 35;

/// Widest the bars are allowed to get. Past a certain point a bar conveys
/// nothing more and simply sprawls across the screen, so the extra room on a
/// wide terminal is left unused. These are the widths dlm has always drawn,
/// which is what a roomy terminal keeps getting - the difference is that the
/// bars now shrink below them instead of wrapping.
const MAX_MAIN_BAR_WIDTH: usize = 133;
const MAX_FILE_BAR_WIDTH: usize = 40;

/// Room the columns beside the bars need at their widest: `{pos}/{len}` for
/// the main bar, and for a download line the elapsed time, transferred/total,
/// speed and ETA at their longest renderings - its file name column is
/// counted separately, being sized from the terminal.
const MAIN_LINE_OVERHEAD: usize = 16;
const DL_LINE_OVERHEAD: usize = 70;

/// Columns available on the terminal, or [`FALLBACK_TERM_WIDTH`] when stdout
/// is not one.
fn terminal_width() -> usize {
    console::Term::stdout()
        .size_checked()
        .map_or(FALLBACK_TERM_WIDTH, |(_rows, cols)| cols as usize)
}

/// Truncate or pad `s` to exactly `width` characters, so the columns after it
/// line up across bars.
fn pad_message(s: &str, width: usize) -> String {
    let count = s.chars().count();
    match count.cmp(&width) {
        Ordering::Greater => s.chars().take(width).collect(),
        Ordering::Equal => s.to_string(),
        Ordering::Less => format!("{}{}", s, " ".repeat(width - count)),
    }
}

/// The transfer rate to display, or `None` when there is no credible estimate.
///
/// The rate estimator restarts from zero every time it is reset - once when
/// the resumed offset is primed, once when the first byte arrives - and ramps
/// up over the following samples. The values it reports on the way up are
/// below one byte per second, which renders as `0 B/s` and, worse, turns into
/// an ETA of thousands of years. A rate that rounds down to zero bytes is not
/// a measurement, so it is reported as "not known yet" instead.
fn known_speed(per_sec: f64) -> Option<u64> {
    if per_sec.is_finite() && per_sec >= 1.0 {
        Some(per_sec as u64)
    } else {
        None
    }
}

pub struct ProgressBarManager {
    mp: MultiProgress,
    /// Width of the filename column, sized once against the terminal.
    msg_width: usize,
    /// Counts the links processed so far. Absent for a single download, where
    /// it could only ever read `0/1` then `1/1` - the file's own bar already
    /// says everything there is to say.
    main_pb: Option<ProgressBar>,
    file_pb_count: u64,
    tx: Sender<ProgressBar>,
    rx: Receiver<ProgressBar>,
}

impl ProgressBarManager {
    pub async fn init(max_concurrent_downloads: u32, main_pb_len: u64) -> Self {
        let mp = MultiProgress::new();
        // Refresh terminal 5 times per seconds
        let draw_target = ProgressDrawTarget::stdout_with_hz(5);
        mp.set_draw_target(draw_target);

        // The layout is chosen against the terminal instead of assuming one at
        // least 140 columns wide. A bar keeps its familiar fixed width while
        // the line has room to spare; once it does not, `{wide_bar}` takes
        // over and shrinks into what is left rather than wrapping.
        let term_width = terminal_width();
        let msg_width = (term_width / 4).clamp(MIN_MSG_WIDTH, MAX_MSG_WIDTH);

        // A fixed width is used whenever the terminal has room for it, so a
        // wide screen keeps the familiar bar instead of one stretched across
        // it; `{wide_bar}` takes over below that, shrinking to fit rather than
        // wrapping.
        let main_bar = if term_width >= MAX_MAIN_BAR_WIDTH + MAIN_LINE_OVERHEAD {
            format!("{{bar:{MAX_MAIN_BAR_WIDTH}}}")
        } else {
            "{wide_bar}".to_string()
        };

        // main progress bar, only worth drawing for more than one link
        let main_pb = (main_pb_len > 1).then(|| {
            let main_style = ProgressStyle::default_bar()
                .template(&format!("{main_bar} {{pos}}/{{len}}"))
                .expect("templating should not fail");
            let main_pb = mp.add(ProgressBar::new(0));
            main_pb.set_style(main_style);
            main_pb.set_length(main_pb_len);
            main_pb
        });

        // `file_pb_count` progress bars are shared between the threads at anytime
        let file_pb_count = min(u64::from(max_concurrent_downloads), main_pb_len);

        let (tx, rx): (Sender<ProgressBar>, Receiver<ProgressBar>) =
            async_channel::bounded(file_pb_count as usize);

        // The bar only takes its fixed width once the line is sure to hold it
        // even when every column renders at its longest - otherwise a file
        // large enough to widen the byte counts would push the line into a
        // second row. Below that `{wide_bar}` shrinks into whatever is left.
        let dl_bar = if term_width >= msg_width + DL_LINE_OVERHEAD + MAX_FILE_BAR_WIDTH {
            format!("{{bar:{MAX_FILE_BAR_WIDTH}.cyan/blue}}")
        } else {
            "{wide_bar:.cyan/blue}".to_string()
        };

        // On a narrow terminal the elapsed time is the first column to go: it
        // is the least useful of them, and dropping it buys the bar room
        // instead of pushing the line into a second row.
        let dl_template = if term_width >= ROOMY_TERM_WIDTH {
            format!(
                "{{msg}} [{{elapsed_precise}}] [{dl_bar}] {{bytes}}/{{total_bytes}} (speed:{{bytes_per_sec}}) (eta:{{eta}})"
            )
        } else {
            format!(
                "{{msg}} [{dl_bar}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}}) (eta:{{eta}})"
            )
        };

        let dl_style = ProgressStyle::default_bar()
            .template(&dl_template)
            .expect("templating should not fail")
            // Until the first byte is received the estimator has no data and the
            // default `{bytes_per_sec}`/`{eta}` would show `0/s` and `eta:0s`,
            // which misleadingly reads as "almost done" while the request is
            // still connecting. Render `--` whenever no real speed is known
            // (before the first byte, and during long stalls).
            .with_key(
                "bytes_per_sec",
                |state: &ProgressState, w: &mut dyn std::fmt::Write| match known_speed(
                    state.per_sec(),
                ) {
                    Some(per_sec) => write!(w, "{}/s", HumanBytes(per_sec)).unwrap(),
                    None => write!(w, "--").unwrap(),
                },
            )
            .with_key(
                "eta",
                |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                    // The ETA is derived from the same estimate, so it is only
                    // worth showing when that estimate is.
                    match known_speed(state.per_sec()) {
                        Some(_) => write!(w, "{:#}", HumanDuration(state.eta())).unwrap(),
                        None => write!(w, "--").unwrap(),
                    }
                },
            )
            .progress_chars("#>-");

        for _ in 0..file_pb_count {
            let file_pb = mp.add(ProgressBar::new(0));
            file_pb.set_style(dl_style.clone());
            file_pb.set_message(pad_message(PENDING, msg_width));
            tx.send(file_pb).await.expect("channel should not fail");
        }

        Self {
            mp,
            msg_width,
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
        if let Some(main_pb) = &self.main_pb {
            main_pb.finish();
        }
        Ok(())
    }

    pub fn increment_global_progress(&self) {
        if let Some(main_pb) = &self.main_pb {
            main_pb.inc(1);
        }
    }

    /// Fit `s` to the filename column of the download bars.
    pub fn message_progress_bar(&self, s: &str) -> String {
        pad_message(s, self.msg_width)
    }

    /// Logging goes through the `MultiProgress` rather than any single bar, so
    /// it lands above whichever bars are on screen - there is not always a
    /// main one to hang it off.
    ///
    /// The timestamp stays uncoloured; only the message carries the level, so
    /// the left margin reads evenly down the log.
    fn log_at(&self, msg: impl std::fmt::Display) {
        let now = Zoned::now().strftime("%Y-%m-%d %H:%M:%S");
        self.print_line(&format!("[{now}] {msg}"));
    }

    /// Print one line above the progress bars. When the bars draw nowhere -
    /// stdout redirected to a file or piped into another command - indicatif
    /// swallows `println`, so the line is written to stdout directly instead;
    /// otherwise a redirected run would lose every log, failures included.
    fn print_line(&self, line: &str) {
        if self.mp.is_hidden() {
            println!("{line}");
        } else {
            let _ = self.mp.println(line);
        }
    }

    /// Ordinary progress: what was downloaded, skipped, or is starting.
    pub fn log_above_progress_bars(&self, msg: &str) {
        self.log_at(msg);
    }

    /// Something the run worked around - a download restarted from scratch, a
    /// server that would not answer properly, a completeness check that could
    /// not be made. Worth noticing, but the run carries on.
    pub fn warn_above_progress_bars(&self, msg: &str) {
        self.log_at(style(msg).yellow());
    }

    /// A link that will not produce a file.
    pub fn error_above_progress_bars(&self, msg: &str) {
        self.log_at(style(msg).red());
    }

    /// Print the end-of-run report, once the progress bars are finished.
    ///
    /// On a terminal it goes through the progress bars so that each line lands
    /// on its own line instead of being appended to a finished bar. Unlike the
    /// running logs it carries no timestamp: it describes the whole run, not a
    /// moment in it.
    pub fn print_report(&self, lines: &[String]) {
        for line in lines {
            self.print_line(line);
        }
    }

    pub async fn claim_progress_bar(&self) -> ProgressBar {
        self.rx
            .recv()
            .await
            .expect("claiming progress bar should not fail")
    }

    pub async fn release_progress_bar(&self, pb: ProgressBar) {
        pb.reset();
        pb.set_message(self.message_progress_bar(PENDING));
        self.tx
            .send(pb)
            .await
            .expect("releasing progress bar should not fail");
    }

    /// Test-only manager that draws nowhere, so unit tests can exercise code
    /// paths that log above the progress bars without touching the terminal.
    #[cfg(test)]
    pub(crate) fn hidden() -> Self {
        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let (tx, rx) = async_channel::bounded(1);
        Self {
            mp,
            msg_width: MAX_MSG_WIDTH,
            main_pb: None,
            file_pb_count: 0,
            tx,
            rx,
        }
    }
}

#[cfg(test)]
mod global_bar_tests {
    use super::ProgressBarManager;

    /// A single link gets no global bar - it could only ever read `0/1` and
    /// then `1/1`, which the file's own bar already says (#467). Anything
    /// more than one link keeps it.
    #[tokio::test]
    async fn the_global_bar_is_only_built_for_more_than_one_link() {
        let single = ProgressBarManager::init(2, 1).await;
        assert!(
            single.main_pb.is_none(),
            "a single download should not get a global bar"
        );

        let several = ProgressBarManager::init(2, 5).await;
        assert!(
            several.main_pb.is_some(),
            "several downloads should be tracked by a global bar"
        );
    }
}

#[cfg(test)]
mod message_width_tests {
    use super::{MAX_MSG_WIDTH, MIN_MSG_WIDTH, pad_message};

    #[test]
    fn short_names_are_padded_to_the_column_width() {
        assert_eq!(pad_message("a.bin", 10), "a.bin     ");
        assert_eq!(pad_message("a.bin", 10).chars().count(), 10);
    }

    #[test]
    fn long_names_are_truncated_to_the_column_width() {
        let long = "a-very-long-file-name-that-does-not-fit.bin";
        assert_eq!(pad_message(long, 10).chars().count(), 10);
        assert_eq!(pad_message(long, 10), "a-very-lon");
    }

    /// Truncation counts characters, not bytes, so a multi-byte name cannot
    /// be cut mid-character and the column stays aligned.
    #[test]
    fn truncation_respects_character_boundaries() {
        let name = "ünïcödé-fïlé-nämé.bin";
        let cut = pad_message(name, 6);
        assert_eq!(cut.chars().count(), 6);
        assert_eq!(cut, "ünïcöd");
    }

    /// The width is derived from the terminal, so the bounds are what keep a
    /// tiny terminal from losing the name and a huge one from wasting a third
    /// of the line on it.
    #[test]
    fn width_bounds_are_sane() {
        for term in [20_usize, 80, 120, 200, 1000] {
            let w = (term / 4).clamp(MIN_MSG_WIDTH, MAX_MSG_WIDTH);
            assert!((MIN_MSG_WIDTH..=MAX_MSG_WIDTH).contains(&w), "term {term}");
        }
    }
}

#[cfg(test)]
mod speed_display_tests {
    use super::known_speed;

    #[test]
    fn a_real_rate_is_displayed() {
        assert_eq!(known_speed(1.0), Some(1));
        assert_eq!(known_speed(1_048_576.0), Some(1_048_576));
    }

    /// The estimator ramping up after a reset reports rates that round down to
    /// zero bytes; showing them yields `0 B/s` and an ETA of millennia (#444).
    #[test]
    fn a_rate_that_rounds_to_zero_is_not_a_measurement() {
        assert_eq!(known_speed(0.0), None);
        assert_eq!(known_speed(0.9), None);
        assert_eq!(known_speed(1e-9), None);
    }

    #[test]
    fn non_finite_rates_are_rejected() {
        assert_eq!(known_speed(f64::NAN), None);
        assert_eq!(known_speed(f64::INFINITY), None);
        assert_eq!(known_speed(f64::NEG_INFINITY), None);
    }
}

#[cfg(test)]
mod recycling_tests {
    use super::*;

    /// Build a manager holding `count` real (but non-drawing) file progress
    /// bars, mirroring `init` without touching the terminal.
    async fn manager_with_bars(count: usize) -> ProgressBarManager {
        let mp = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let main_pb = mp.add(ProgressBar::hidden());
        let (tx, rx) = async_channel::bounded(count.max(1));
        for _ in 0..count {
            let pb = mp.add(ProgressBar::hidden());
            pb.set_message(pad_message(PENDING, MAX_MSG_WIDTH));
            tx.send(pb).await.unwrap();
        }
        ProgressBarManager {
            mp,
            msg_width: MAX_MSG_WIDTH,
            main_pb: Some(main_pb),
            file_pb_count: count as u64,
            tx,
            rx,
        }
    }

    /// A bar that already served a (resumed) download must come back from the
    /// recycle pool with no leftover position or speed, so the next download
    /// starts from a clean slate and renders `--` until its own first byte.
    #[tokio::test]
    async fn recycled_bar_carries_no_position_or_speed() {
        let mgr = manager_with_bars(1).await;

        // simulate a full download on the claimed bar
        let pb = mgr.claim_progress_bar().await;
        pb.set_length(1000);
        pb.set_position(400); // resumed offset
        pb.inc(600); // streamed to completion
        assert_eq!(pb.position(), 1000);

        // recycle it back to the pool
        mgr.release_progress_bar(pb).await;

        // the next download claims the same underlying bar
        let pb_next = mgr.claim_progress_bar().await;
        assert_eq!(
            pb_next.position(),
            0,
            "recycled bar must not carry the previous download's position"
        );
        assert_eq!(
            pb_next.per_sec(),
            0.0,
            "recycled bar must report no speed (renders as `--`) until its first byte"
        );
    }

    /// With more downloads than bars, every release must hand back a clean bar
    /// for the queued downloads to claim.
    #[tokio::test]
    async fn bar_stays_clean_across_several_recycles() {
        let mgr = manager_with_bars(1).await;

        for offset in [100_u64, 250, 700] {
            let pb = mgr.claim_progress_bar().await;
            pb.set_length(1000);
            pb.set_position(offset);
            pb.inc(1000 - offset);
            mgr.release_progress_bar(pb).await;

            let pb_check = mgr.claim_progress_bar().await;
            assert_eq!(pb_check.position(), 0);
            assert_eq!(pb_check.per_sec(), 0.0);
            mgr.release_progress_bar(pb_check).await;
        }
    }
}
