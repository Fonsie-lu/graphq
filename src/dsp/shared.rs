//! State shared between the audio thread and the editor: analyser taps,
//! output meters and the automatic EQ capture. The audio thread only stores
//! into atomics; the mutexes here are taken by the GUI thread alone.

use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::auto::{AutoError, Capture, Feature};
use super::filter::Band;

const RELAXED: Ordering = Ordering::Relaxed;

pub struct Shared {
    /// Mono sum before and after the EQ, for the spectrum analyser.
    pub pre: AnalyzerRing,
    pub post: AnalyzerRing,
    pub sample_rate: AtomicF32,
    /// Set while the editor is open; the audio thread skips the taps otherwise.
    pub gui_open: AtomicBool,
    /// Output peak per channel since the editor last took it.
    pub peak: [AtomicF32; 2],
    /// Capture the audio thread fills, or null. Points into one of
    /// `captures`, which keeps every buffer alive as long as `Shared`, so the
    /// audio thread never frees one. Buffers are reused; a new one is only
    /// allocated (on the GUI thread) for a new sample rate.
    capture: AtomicPtr<Capture>,
    captures: Mutex<Vec<Arc<Capture>>>,
    /// The automatic EQ run in progress and its last status (written by the
    /// GUI thread only; here so it survives closing the editor).
    pub job: Mutex<AutoJob>,
    /// Bands from the analysis thread, tagged with the run they belong to.
    pub result: Mutex<Option<(u64, AutoResult)>>,
}

/// What an automatic EQ analysis returns.
pub type AutoResult = Result<Vec<Band>, AutoError>;

#[derive(Clone, Debug, Default)]
pub struct AutoJob {
    pub running: Option<Feature>,
    /// Counts runs, so a result from a cancelled run is ignored.
    pub run: u64,
    /// The capture is complete and the analysis thread is working.
    pub analyzing: bool,
    pub status: String,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            pre: AnalyzerRing::default(),
            post: AnalyzerRing::default(),
            sample_rate: AtomicF32::new(48_000.0),
            gui_open: AtomicBool::new(false),
            peak: [AtomicF32::new(0.0), AtomicF32::new(0.0)],
            capture: AtomicPtr::new(std::ptr::null_mut()),
            captures: Mutex::new(Vec::new()),
            job: Mutex::new(AutoJob::default()),
            result: Mutex::new(None),
        }
    }
}

impl Shared {
    /// Raises the held output peaks (audio thread).
    pub fn add_peaks(&self, left: &[f32], right: &[f32]) {
        for (ch, x) in [left, right].into_iter().enumerate() {
            let p = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            if p > self.peak[ch].load(RELAXED) {
                self.peak[ch].store(p, RELAXED);
            }
        }
    }

    /// Feeds the running capture, if any (audio thread).
    pub fn push_capture(&self, left: &[f32], right: &[f32]) {
        let p = self.capture.load(Ordering::Acquire);
        if !p.is_null() {
            // Safety: `p` points into an `Arc` that `captures` holds until
            // `Shared` drops, and the audio thread never outlives `Shared`.
            unsafe { (*p).push_stereo(left, right) };
        }
    }

    /// Starts a new capture at `sample_rate` (GUI thread).
    pub fn start_capture(&self, sample_rate: f32) -> Arc<Capture> {
        self.stop_capture();
        let mut captures = self.captures.lock().unwrap();
        let c = match captures.iter().find(|c| c.sample_rate == sample_rate) {
            Some(c) => c.clone(),
            None => {
                let c = Arc::new(Capture::new(sample_rate));
                captures.push(c.clone());
                c
            }
        };
        c.reset();
        self.capture.store(Arc::as_ptr(&c) as *mut Capture, Ordering::Release);
        c
    }

    /// Detaches the capture from the audio thread (GUI thread).
    pub fn stop_capture(&self) {
        self.capture.store(std::ptr::null_mut(), Ordering::Release);
    }

    /// The capture the audio thread is filling.
    pub fn active_capture(&self) -> Option<Arc<Capture>> {
        let p = self.capture.load(Ordering::Acquire);
        self.captures.lock().unwrap().iter().find(|c| std::ptr::eq(Arc::as_ptr(c), p)).cloned()
    }

    /// Starts an automatic EQ run: records the next 10 s of input (GUI thread).
    pub fn begin_run(&self, feature: Feature) {
        self.start_capture(self.sample_rate.load(RELAXED));
        let mut job = self.job.lock().unwrap();
        job.running = Some(feature);
        job.run += 1;
        job.analyzing = false;
        job.status.clear();
    }

    pub fn cancel_run(&self) {
        self.stop_capture();
        let mut job = self.job.lock().unwrap();
        job.running = None;
        job.analyzing = false;
        job.status = "CANCELLED".to_owned();
    }

    /// Advances a run; call every frame from the GUI thread. When the capture
    /// is full, the analysis starts on its own thread with the bands
    /// `others(feature)` returns (the active bands the feature does not own).
    /// When it is done, the result is returned once; apply it, then call
    /// [`Shared::finish_run`].
    pub fn poll_run(
        self: &Arc<Self>,
        others: impl FnOnce(Feature) -> Vec<Band>,
    ) -> Option<(Feature, AutoResult)> {
        let (feature, run, analyzing) = {
            let job = self.job.lock().unwrap();
            (job.running?, job.run, job.analyzing)
        };
        if analyzing {
            let result = self.result.lock().unwrap().take();
            return result.filter(|(r, _)| *r == run).map(|(_, result)| (feature, result));
        }
        match self.active_capture() {
            Some(c) if c.is_full() => {
                self.stop_capture();
                self.job.lock().unwrap().analyzing = true;
                let (samples, sample_rate) = (c.samples(), c.sample_rate);
                let others = others(feature);
                let shared = self.clone();
                std::thread::spawn(move || {
                    let result = super::auto::analyze(feature, &samples, sample_rate, &others);
                    *shared.result.lock().unwrap() = Some((run, result));
                });
            }
            Some(_) => {}
            None => self.job.lock().unwrap().running = None,
        }
        None
    }

    /// Ends the run whose result was applied, with its status line.
    pub fn finish_run(&self, status: String) {
        let mut job = self.job.lock().unwrap();
        job.running = None;
        job.analyzing = false;
        job.status = status;
    }

    /// What to show for the run: the feature, the recorded share (None while
    /// analysing) and the last status line.
    pub fn run_state(&self) -> (Option<Feature>, Option<f32>, String) {
        let job = self.job.lock().unwrap().clone();
        let progress = match job.running {
            Some(_) if !job.analyzing => self.active_capture().map(|c| c.progress()),
            _ => None,
        };
        (job.running, progress, job.status)
    }

    /// Returns and clears the held peaks (editor).
    pub fn take_peaks(&self) -> [f32; 2] {
        [self.peak[0].swap(0.0, RELAXED), self.peak[1].swap(0.0, RELAXED)]
    }
}

/// Single-producer ring buffer feeding the analyser. Samples are atomics so a
/// reader racing the writer sees old or new values, never torn ones.
pub struct AnalyzerRing {
    data: Box<[AtomicF32]>,
    write: AtomicUsize,
}

impl AnalyzerRing {
    pub const SIZE: usize = 8192;

    /// Pushes the mono sum of a stereo block.
    pub fn push_stereo(&self, left: &[f32], right: &[f32]) {
        let mut w = self.write.load(RELAXED);
        for (l, r) in left.iter().zip(right) {
            self.data[w].store((l + r) * 0.5, RELAXED);
            w = (w + 1) & (Self::SIZE - 1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Copies the most recent `dest.len()` samples in chronological order.
    pub fn copy_latest(&self, dest: &mut [f32]) {
        let n = dest.len().min(Self::SIZE);
        let w = self.write.load(Ordering::Acquire);
        for (i, d) in dest.iter_mut().take(n).enumerate() {
            let idx = (w + Self::SIZE - n + i) & (Self::SIZE - 1);
            *d = self.data[idx].load(RELAXED);
        }
    }
}

impl Default for AnalyzerRing {
    fn default() -> Self {
        Self {
            data: (0..Self::SIZE).map(|_| AtomicF32::new(0.0)).collect(),
            write: AtomicUsize::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_returns_latest_in_order() {
        let ring = AnalyzerRing::default();
        let input: Vec<f32> = (0..10_000).map(|i| i as f32).collect();
        ring.push_stereo(&input, &input);
        let mut out = [0.0; 4];
        ring.copy_latest(&mut out);
        assert_eq!(out, [9996.0, 9997.0, 9998.0, 9999.0]);
    }

    #[test]
    fn capture_buffers_are_reused_per_sample_rate() {
        let s = Shared::default();
        let a = s.start_capture(1000.0);
        s.push_capture(&[1.0; 300], &[1.0; 300]);
        assert_eq!(a.samples().len(), 300);
        let b = s.start_capture(1000.0);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(b.samples().len(), 0);
        s.stop_capture();
        s.push_capture(&[1.0; 300], &[1.0; 300]);
        assert_eq!(b.samples().len(), 0);
        assert!(s.active_capture().is_none());
        let c = s.start_capture(2000.0);
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(Arc::ptr_eq(&s.active_capture().unwrap(), &c));
    }

    #[test]
    fn peaks_hold_until_taken() {
        let s = Shared::default();
        s.add_peaks(&[0.1, -0.7], &[0.2]);
        s.add_peaks(&[0.3], &[0.1]);
        assert_eq!(s.take_peaks(), [0.7, 0.2]);
        assert_eq!(s.take_peaks(), [0.0, 0.0]);
    }
}
