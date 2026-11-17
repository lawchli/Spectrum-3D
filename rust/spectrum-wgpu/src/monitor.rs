//! Performance readout of the native window, the counterpart of web/src/monitor.js:
//! frame pacing, the spectrum pass on the GPU, CPU time per frame, this process and the
//! audio path. Collecting is a few additions per frame; drawing happens only while shown.
use spectrum_wgpu::audio::AudioStats;
use std::{collections::VecDeque, time::Instant};

/// Frame intervals kept for the chart and the statistics: four seconds at 60 Hz.
const HISTORY: usize = 240;
/// A frame counts as dropped when it took this many refresh periods or more.
const DROP_AT: f32 = 1.5;

pub struct Monitor {
    intervals: VecDeque<f32>,
    dropped: u64,
    budget_ms: f32,
    upload_ms: f32,
    draw_ms: f32,
    started: Instant,
    /// When the system was last asked; `None` until the first frame, which asks at once.
    usage_at: Option<Instant>,
    usage_cpu: Option<f64>,
    cpu_percent: Option<f32>,
    rss: Option<u64>,
    peak_rss: Option<u64>,
    memory: MemoryHistory,
}

/// One column of the monitor dock, as web/src/monitor.js builds them: a title, a note
/// beside it (the GPU, the core count), the headline figure on the right, then label and
/// value pairs with the values right-aligned.
pub struct Column {
    pub part: Part,
    pub title: String,
    pub note: String,
    pub value: String,
    pub rows: Vec<(String, String)>,
}
/// The five columns, in the web order. Which ones show is chosen in the panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Frame,
    Gpu,
    Cpu,
    Memory,
    Audio,
}
impl Part {
    pub const ALL: [Part; 5] = [Part::Frame, Part::Gpu, Part::Cpu, Part::Memory, Part::Audio];
    /// The key the web settings use for it.
    pub fn key(self) -> &'static str {
        match self {
            Part::Frame => "frame",
            Part::Gpu => "gpu",
            Part::Cpu => "cpu",
            Part::Memory => "memory",
            Part::Audio => "audio",
        }
    }
}

/// Everything the dock shows, read out before the interface borrows the renderer.
pub struct View {
    pub intervals: Vec<f32>,
    pub budget_ms: f32,
    pub frozen: Option<String>,
    pub columns: Vec<Column>,
    /// Resident memory readings: seconds before now, and MB.
    pub memory: Vec<(f32, f32)>,
    /// The top of the memory chart, in MB.
    pub memory_top: f32,
}

/// What the system says about this process, read about once a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Usage {
    /// CPU seconds used so far, user and system together.
    cpu: Option<f64>,
    /// Resident memory now, in bytes.
    resident: Option<u64>,
    /// The most resident memory since the process started, in bytes.
    peak: Option<u64>,
}

/// CPU seconds and peak resident size from `getrusage`.
#[cfg(all(unix, target_pointer_width = "64"))]
fn rusage() -> Option<(f64, u64)> {
    // struct rusage on 64-bit macOS and Linux: two timevals, then fourteen longs of which
    // ru_maxrss is the first. Declared here so the readout needs no extra crate.
    #[repr(C)]
    struct Rusage {
        user: [i64; 2],
        system: [i64; 2],
        max_rss: i64,
        rest: [i64; 13],
    }
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut usage = std::mem::MaybeUninit::<Rusage>::zeroed();
    // SAFETY: the struct has the size and layout the C library writes; 0 is RUSAGE_SELF.
    let usage = unsafe {
        if getrusage(0, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };
    // tv_usec is 32 bits wide on macOS and 64 on Linux, and below a million on both.
    let seconds = |t: [i64; 2]| t[0] as f64 + (t[1] & 0xffff_ffff) as f64 / 1e6;
    // macOS reports bytes, Linux kilobytes.
    let scale = if cfg!(target_os = "macos") { 1 } else { 1024 };
    Some((
        seconds(usage.user) + seconds(usage.system),
        usage.max_rss.max(0) as u64 * scale,
    ))
}

/// Resident size now, from the task's basic info (what `getrusage` reports the peak of).
#[cfg(target_os = "macos")]
fn resident() -> Option<u64> {
    // struct mach_task_basic_info: three 64-bit sizes, two time_value_t, policy and
    // suspend count; MACH_TASK_BASIC_INFO_COUNT is its size in 32-bit words.
    #[repr(C)]
    #[derive(Default)]
    struct Info {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [i32; 2],
        system_time: [i32; 2],
        policy: i32,
        suspend_count: i32,
    }
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
    }
    const MACH_TASK_BASIC_INFO: u32 = 20;
    let mut info = Info::default();
    let mut count = (std::mem::size_of::<Info>() / 4) as u32;
    // SAFETY: the buffer is as large as the count says; mach_task_self_ is this task's port.
    let status = unsafe {
        task_info(
            mach_task_self_,
            MACH_TASK_BASIC_INFO,
            (&mut info as *mut Info).cast(),
            &mut count,
        )
    };
    (status == 0).then_some(info.resident_size)
}

/// Resident size now: the second field of /proc/self/statm, in pages.
#[cfg(target_os = "linux")]
fn resident() -> Option<u64> {
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    const SC_PAGESIZE: i32 = 30;
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf only reads a constant.
    let page = unsafe { sysconf(SC_PAGESIZE) };
    (page > 0).then(|| pages * page as u64)
}

#[cfg(all(unix, target_pointer_width = "64"))]
fn usage() -> Usage {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let resident = resident();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let resident = None;
    let (cpu, peak) = rusage().unzip();
    Usage {
        cpu,
        resident,
        peak,
    }
}

/// The working set (Task Manager's memory column) and the process's CPU times.
#[cfg(windows)]
fn usage() -> Usage {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    let (mut creation, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo handle needs no closing; each buffer is the size it is declared as.
    let (memory, times) = unsafe {
        let process = GetCurrentProcess();
        (
            K32GetProcessMemoryInfo(process, &mut counters, counters.cb) != 0,
            GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0,
        )
    };
    // FILETIME counts 100 ns intervals.
    let seconds = |t: &FileTime| ((t.high as u64) << 32 | t.low as u64) as f64 / 1e7;
    Usage {
        cpu: times.then(|| seconds(&kernel) + seconds(&user)),
        resident: memory.then_some(counters.working_set_size as u64),
        peak: memory.then_some(counters.peak_working_set_size as u64),
    }
}

#[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
fn usage() -> Usage {
    Usage::default()
}

/// Seconds of memory history the chart shows.
pub const MEMORY_WINDOW_S: f64 = 120.;
/// Samples further apart than this are not joined: the monitor was off or the app stalled.
pub const MEMORY_GAP_S: f64 = 2.5;

/// Resident memory over the last two minutes, in MB, against the time it was read.
#[derive(Default)]
struct MemoryHistory {
    samples: VecDeque<(f64, f64)>,
}
impl MemoryHistory {
    fn push(&mut self, at: f64, mb: f64) {
        self.samples.push_back((at, mb));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| *t < at - MEMORY_WINDOW_S)
        {
            self.samples.pop_front();
        }
    }
    /// The top of the chart: the step above the most in the window, with a tenth to spare,
    /// so the scale only moves when a reading passes it or the last high one has scrolled out.
    fn top(&self) -> f64 {
        nice_ceiling(self.samples.iter().map(|(_, mb)| *mb).fold(0., f64::max) * 1.1)
    }
}

/// The smallest of 1, 1.5, 2, 3, 4, 5, 6, 8 times a power of ten at or above `value`.
pub fn nice_ceiling(value: f64) -> f64 {
    if !(value > 1.) {
        return 1.;
    }
    let mut power = 10f64.powi(value.log10().floor() as i32);
    loop {
        for step in [1., 1.5, 2., 3., 4., 5., 6., 8.] {
            if step * power >= value * (1. - 1e-9) {
                return step * power;
            }
        }
        power *= 10.;
    }
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            intervals: VecDeque::with_capacity(HISTORY),
            dropped: 0,
            budget_ms: 1000. / 60.,
            upload_ms: 0.,
            draw_ms: 0.,
            started: Instant::now(),
            usage_at: None,
            usage_cpu: None,
            cpu_percent: None,
            rss: None,
            peak_rss: None,
            memory: MemoryHistory::default(),
        }
    }
    /// Starts counting afresh, for when the panel is switched on.
    pub fn reset(&mut self) {
        *self = Self {
            budget_ms: self.budget_ms,
            ..Self::new()
        };
    }
    pub fn set_refresh(&mut self, hz: Option<f32>) {
        self.budget_ms = 1000. / hz.filter(|hz| *hz >= 1.).unwrap_or(60.);
    }
    /// One displayed frame: the time since the previous one, and the CPU time this frame
    /// spent uploading rows and encoding the draw.
    pub fn frame(&mut self, interval: f64, upload_ms: f64, draw_ms: f64) {
        let ms = (interval * 1000.) as f32;
        if self.intervals.len() == HISTORY {
            self.intervals.pop_front();
        }
        self.intervals.push_back(ms);
        if ms >= self.budget_ms * DROP_AT {
            self.dropped += 1;
        }
        self.upload_ms += (upload_ms as f32 - self.upload_ms) * 0.1;
        self.draw_ms += (draw_ms as f32 - self.draw_ms) * 0.1;
        let elapsed = self.usage_at.map(|at| at.elapsed().as_secs_f64());
        if elapsed.is_none_or(|e| e >= 1.) {
            let usage = usage();
            if let (Some(cpu), Some(before), Some(elapsed)) = (usage.cpu, self.usage_cpu, elapsed) {
                self.cpu_percent = Some(((cpu - before) / elapsed * 100.) as f32);
            }
            self.usage_cpu = usage.cpu;
            self.rss = usage.resident;
            self.peak_rss = usage.peak;
            if let Some(rss) = usage.resident {
                self.memory
                    .push(self.started.elapsed().as_secs_f64(), rss as f64 / 1048576.);
            }
            self.usage_at = Some(Instant::now());
        }
    }
    /// The same numbers as the panel, for the line a `--frames` run prints on exit, so an
    /// acceptance run can read them without looking at a picture. The frame figures cover
    /// the last 240 frames; the drop count covers the whole run since the monitor opened.
    pub fn summary(&self) -> serde_json::Value {
        let n = self.intervals.len();
        let mean = self.intervals.iter().sum::<f32>() / n.max(1) as f32;
        let mut sorted: Vec<f32> = self.intervals.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let slow = sorted
            .get((n as f32 * 0.99).ceil() as usize - usize::from(n > 0))
            .copied()
            .unwrap_or(0.);
        serde_json::json!({
            "fps": if mean > 0. { 1000. / mean } else { 0. },
            "frameMs": mean, "slowest1PercentMs": slow, "dropped": self.dropped,
            "refreshMs": self.budget_ms, "uploadMs": self.upload_ms, "drawMs": self.draw_ms,
            "processCpuPercent": self.cpu_percent, "rssBytes": self.rss, "peakRssBytes": self.peak_rss,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn view(
        &self,
        t: impl Fn(&str) -> String,
        gpu_ms: Option<f64>,
        gpu_timer: bool,
        adapter: &str,
        (vertices, calls): (u64, u32),
        render_dims: [u32; 2],
        msaa: u32,
        render_scale: f32,
        scale_mode: &str,
        audio: Option<&AudioStats>,
        frozen: bool,
    ) -> View {
        let n = self.intervals.len();
        let mean = self.intervals.iter().sum::<f32>() / n.max(1) as f32;
        let mut sorted: Vec<f32> = self.intervals.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let slow = sorted
            .get((n as f32 * 0.99).ceil() as usize - usize::from(n > 0))
            .copied()
            .unwrap_or(0.);
        let ms = |v: f64| format!("{v:.2} ms");
        let column = |part, title: String, note: String, value: String, rows| Column {
            part,
            title,
            note,
            value,
            rows,
        };
        let mut columns = vec![column(
            Part::Frame,
            t("monFrame"),
            String::new(),
            format!("{:.0} fps", if mean > 0. { 1000. / mean } else { 0. }),
            vec![
                (t("mon.avg"), format!("{mean:.1} ms")),
                (t("mon.worst"), format!("{slow:.1} ms")),
                (t("mon.drop"), self.dropped.to_string()),
            ],
        )];
        columns.push(column(
            Part::Gpu,
            "GPU".into(),
            adapter.to_owned(),
            match (gpu_ms, gpu_timer) {
                (Some(v), _) => ms(v),
                (None, true) => t("mon.gpuTiming"),
                (None, false) => t("mon.gpuNoTimer"),
            },
            vec![
                (
                    String::new(),
                    format!(
                        "{}\u{d7}{}{}  {}%{}",
                        render_dims[0],
                        render_dims[1],
                        if msaa > 1 {
                            format!(" {msaa}x")
                        } else {
                            String::new()
                        },
                        (render_scale * 100.0).round() as u32,
                        if scale_mode == "auto" { " A" } else { "" }
                    ),
                ),
                (t("mon.verts"), format!("{:.0}k", vertices as f64 / 1000.)),
                (t("mon.draws"), calls.to_string()),
            ],
        ));
        let mut cpu = vec![];
        if let Some(audio) = audio {
            cpu.push((t("mon.analysis"), ms(audio.analysis_ms)));
        }
        cpu.push((t("mon.upload"), ms(self.upload_ms as f64)));
        cpu.push((t("mon.draw"), ms(self.draw_ms as f64)));
        if let Some(percent) = self.cpu_percent {
            cpu.push((t("mon.process"), format!("{percent:.0}%")));
        }
        columns.push(column(
            Part::Cpu,
            "CPU".into(),
            std::thread::available_parallelism()
                .map(|c| t("mon.cores").replace("{n}", &c.to_string()))
                .unwrap_or_default(),
            ms((self.upload_ms + self.draw_ms) as f64),
            cpu,
        ));
        // Native has the process's resident memory where the web has the JS heap.
        let mb = |bytes: u64| format!("{:.0} MB", bytes as f64 / 1048576.);
        columns.push(column(
            Part::Memory,
            t("mon.memTitle"),
            t("mon.resident"),
            self.rss.map(mb).unwrap_or_else(|| "\u{2014}".into()),
            vec![(
                t("mon.peak"),
                self.peak_rss.map(mb).unwrap_or_else(|| "\u{2014}".into()),
            )],
        ));
        columns.push(match audio {
            None => column(
                Part::Audio,
                t("monAudio"),
                String::new(),
                "\u{2014}".into(),
                vec![(String::new(), t("mon.notStarted"))],
            ),
            Some(a) => column(
                Part::Audio,
                t("monAudio"),
                String::new(),
                // Analysis always runs at 48 kHz; another device rate is resampled to it.
                if a.source_sample_rate != 0 && a.source_sample_rate != 48000 {
                    format!("{:.1} > 48.0 kHz", a.source_sample_rate as f64 / 1000.)
                } else {
                    "48.0 kHz".into()
                },
                vec![
                    (
                        String::new(),
                        t("mon.rowsPerSec").replace("{rows}", &format!("{:.0}", a.rows_per_second)),
                    ),
                    (
                        t("mon.lat"),
                        a.capture_to_submit_ms.map(ms).unwrap_or_else(|| "—".into()),
                    ),
                    (
                        t("mon.callback"),
                        a.callback_frames
                            .map(|n| {
                                format!(
                                    "{n} / {:.1} ms",
                                    n as f64 * 1000.0 / a.source_sample_rate.max(1) as f64
                                )
                            })
                            .unwrap_or_else(|| "—".into()),
                    ),
                    ("FFT".into(), format!("{:.0}/s", a.fft_rows_per_second)),
                    (t("mon.lostInput"), a.input_frames_dropped.to_string()),
                    (t("mon.lostRows"), a.display_rows_dropped.to_string()),
                ],
            ),
        });
        View {
            intervals: self.intervals.iter().copied().collect(),
            budget_ms: self.budget_ms,
            frozen: frozen.then(|| t("mon.frozen")),
            columns,
            memory: {
                let now = self.started.elapsed().as_secs_f64();
                self.memory
                    .samples
                    .iter()
                    .map(|(at, mb)| ((now - at) as f32, *mb as f32))
                    .collect()
            },
            memory_top: self.memory.top() as f32,
        }
    }
}

/// Colours of web/style.css, shared with the panel.
pub mod ink {
    use egui::Color32;
    pub const BG_WELL: Color32 = Color32::from_rgb(0x07, 0x08, 0x0a);
    pub const BG_PANEL: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x12);
    pub const BG_CONTROL: Color32 = Color32::from_rgb(0x13, 0x16, 0x1c);
    pub const BG_RAISED: Color32 = Color32::from_rgb(0x1c, 0x20, 0x29);
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x27);
    pub const BORDER_MID: Color32 = Color32::from_rgb(0x28, 0x2e, 0x39);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3e, 0x47, 0x56);
    pub const INK_1: Color32 = Color32::from_rgb(0xea, 0xed, 0xf2);
    pub const INK_2: Color32 = Color32::from_rgb(0x8e, 0x97, 0xa4);
    pub const INK_3: Color32 = Color32::from_rgb(0x54, 0x5d, 0x6a);
    pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xa2, 0x46);
}

/// Height of one line of text in the dock.
const LINE: f32 = 14.;
/// Room on each side of the hairline between two columns, and above and below the one
/// between two rows.
const GAP: f32 = 12.;
/// The narrowest a chart is drawn.
const CHART_MIN: f32 = 96.;
/// A column with more label and value rows than this lists them in two halves side by
/// side (the audio column), so that it is no taller than the others.
const SPLIT_ROWS: usize = 4;

/// Splits `widths.len()` columns, in order, into the fewest rows whose columns fit in
/// `room` side by side, `gap` apart. Earlier rows take the extra column: five go three
/// over two. Returns how many columns each row has.
pub fn pack(widths: &[f32], room: f32, gap: f32) -> Vec<usize> {
    let n = widths.len();
    for rows in 1..=n {
        let counts: Vec<usize> = (0..rows)
            .map(|r| n / rows + usize::from(r < n % rows))
            .collect();
        let mut start = 0;
        let fits = counts.iter().all(|&count| {
            let row = &widths[start..start + count];
            start += count;
            row.iter().sum::<f32>() + gap * count.saturating_sub(1) as f32 <= room
        });
        if fits {
            return counts;
        }
    }
    vec![1; n]
}

/// Widths of one row's columns filling `room`: each its least, and what is left over
/// shared out by weight. A row that does not fit keeps the least widths.
pub fn spread(least: &[f32], weights: &[f32], room: f32, gap: f32) -> Vec<f32> {
    let spare = room - least.iter().sum::<f32>() - gap * least.len().saturating_sub(1) as f32;
    let total: f32 = weights.iter().sum();
    least
        .iter()
        .zip(weights)
        .map(|(l, w)| {
            l + if spare > 0. && total > 0. {
                spare * w / total
            } else {
                0.
            }
        })
        .collect()
}

/// The frame and memory columns draw a chart right of their figures.
fn has_chart(part: Part) -> bool {
    matches!(part, Part::Frame | Part::Memory)
}

fn headline_font(part: Part) -> egui::FontId {
    if part == Part::Frame {
        egui::FontId::monospace(16.)
    } else {
        egui::FontId::monospace(crate::fonts::SMALL)
    }
}

/// How much room a column's text needs, measured in the fonts it is drawn with.
struct Measure {
    /// Width of the figures, and of the whole column with its chart at the narrowest.
    text: f32,
    least: f32,
    /// Lines from the title down.
    lines: usize,
    /// Where the rows divide into a second list, and the two lists' widths.
    split: Option<(usize, f32, f32)>,
}

fn measure(p: &egui::Painter, c: &Column, sans: &egui::FontId, mono: &egui::FontId) -> Measure {
    let w = |text: &str, font: &egui::FontId| {
        p.layout_no_wrap(text.to_owned(), font.clone(), ink::INK_1)
            .size()
            .x
    };
    // The note may be cut short; it keeps a few characters at the least.
    let note = if c.note.is_empty() {
        0.
    } else {
        6. + w(&c.note, sans).min(64.)
    };
    let head = w(&c.title, sans) + note + 14. + w(&c.value, &headline_font(c.part));
    let list = |rows: &[(String, String)]| {
        rows.iter()
            .map(|(label, value)| {
                w(label, sans) + if label.is_empty() { 0. } else { 10. } + w(value, mono)
            })
            .fold(0., f32::max)
    };
    let split = (c.rows.len() > SPLIT_ROWS).then(|| {
        let k = c.rows.len().div_ceil(2);
        (k, list(&c.rows[..k]), list(&c.rows[k..]))
    });
    let rows = match split {
        Some((_, a, b)) => a + 2. * GAP + b,
        None => list(&c.rows),
    };
    let text = head.max(rows).ceil();
    Measure {
        text,
        least: text + if has_chart(c.part) { GAP + CHART_MIN } else { 0. },
        lines: 1 + split.map_or(c.rows.len(), |(k, _, _)| k),
        split,
    }
}

/// The dock's columns, in the web order, in as few rows as the width allows: one row when
/// they fit, else three over two, and so on. Every row fills the width, its columns as
/// tall as each other with a hairline between them. `parts` are the ones switched on.
pub fn dock(ui: &mut egui::Ui, view: &View, parts: &[Part]) {
    use egui::{pos2, vec2, FontId, Rect, Stroke};
    let mono = FontId::monospace(crate::fonts::SMALL);
    let sans = FontId::proportional(crate::fonts::SMALL);
    let columns: Vec<&Column> = view
        .columns
        .iter()
        .filter(|c| parts.contains(&c.part))
        .collect();
    if columns.is_empty() {
        return;
    }
    let mut measures: Vec<Measure> = columns
        .iter()
        .map(|c| measure(ui.painter(), c, &sans, &mono))
        .collect();
    // Each column keeps the widest its figures have been, so a reading that gains a digit
    // does not move the charts or tip the dock between one row and two.
    ui.data_mut(|d| {
        let widest = d.get_temp_mut_or_default::<[f32; 5]>(egui::Id::new("monitor-text-width"));
        for (c, m) in columns.iter().zip(&mut measures) {
            let i = c.part as usize;
            widest[i] = widest[i].max(m.text);
            m.least += widest[i] - m.text;
            m.text = widest[i];
        }
    });
    let least: Vec<f32> = measures.iter().map(|m| m.least).collect();
    let room = ui.available_width();
    let between = 2. * GAP + 1.;
    let mut start = 0;
    let rows: Vec<(std::ops::Range<usize>, f32)> = pack(&least, room, between)
        .into_iter()
        .map(|count| {
            let range = start..start + count;
            start += count;
            let lines = measures[range.clone()]
                .iter()
                .map(|m| m.lines)
                .max()
                .unwrap_or(1);
            (range, (LINE * lines as f32 + 4.).max(48.))
        })
        .collect();
    let height = rows.iter().map(|(_, h)| h).sum::<f32>() + (rows.len() - 1) as f32 * between;
    let (rect, _) = ui.allocate_exact_size(vec2(room, height), egui::Sense::hover());
    // A little over the rect: the frame rate's larger figures reach above the first line.
    let p = ui.painter_at(rect.expand(4.));
    let hairline = Stroke::new(1., ink::BORDER_SUBTLE);
    let mut top = rect.top();
    for (i, (range, h)) in rows.iter().enumerate() {
        if i > 0 {
            p.hline(rect.x_range(), top + GAP + 0.5, hairline);
            top += between;
        }
        let weights: Vec<f32> = columns[range.clone()]
            .iter()
            .map(|c| if has_chart(c.part) { 4. } else { 1. })
            .collect();
        let widths = spread(&least[range.clone()], &weights, room, between);
        let mut left = rect.left();
        for (k, index) in range.clone().enumerate() {
            if k > 0 {
                p.vline(left + GAP + 0.5, top..=top + h, hairline);
                left += between;
            }
            let cell = Rect::from_min_size(pos2(left, top), vec2(widths[k], *h));
            column(&p, cell, columns[index], &measures[index], view, &sans, &mono);
            left += widths[k];
        }
        top += h;
    }
}

/// One column: the title, a note and the headline figure, the label and value rows under
/// them, and for the frame and memory columns a chart to their right.
fn column(
    p: &egui::Painter,
    cell: egui::Rect,
    c: &Column,
    m: &Measure,
    view: &View,
    sans: &egui::FontId,
    mono: &egui::FontId,
) {
    use egui::{pos2, Align2};
    let left = cell.left();
    let right = if has_chart(c.part) {
        left + m.text
    } else {
        cell.right()
    };
    let top = cell.top() + LINE / 2.;
    let title = p.text(
        pos2(left, top),
        Align2::LEFT_CENTER,
        &c.title,
        sans.clone(),
        ink::INK_3,
    );
    let value = p.text(
        pos2(right, top),
        Align2::RIGHT_CENTER,
        &c.value,
        headline_font(c.part),
        ink::INK_1,
    );
    if !c.note.is_empty() {
        // The note gives way to the headline: cut to the room between them.
        let room = value.left() - title.right() - 14.;
        let mut note = c.note.clone();
        while note.chars().count() > 1
            && p.layout_no_wrap(note.clone(), sans.clone(), ink::INK_2)
                .size()
                .x
                > room
        {
            note.pop();
            note.pop();
            note.push('\u{2026}');
        }
        p.text(
            pos2(title.right() + 6., top),
            Align2::LEFT_CENTER,
            note,
            sans.clone(),
            ink::INK_2,
        );
    }
    let list = |rows: &[(String, String)], from: f32, to: f32| {
        for (i, (label, value)) in rows.iter().enumerate() {
            let y = top + LINE * (i as f32 + 1.) + 4.;
            p.text(
                pos2(from, y),
                Align2::LEFT_CENTER,
                label,
                sans.clone(),
                ink::INK_3,
            );
            p.text(
                pos2(to, y),
                Align2::RIGHT_CENTER,
                value,
                mono.clone(),
                ink::INK_2,
            );
        }
    };
    match m.split {
        Some((k, a, b)) => {
            // The second list starts where the first one's share of any spare room ends.
            let spare = (right - left - a - b - 2. * GAP).max(0.);
            let middle = left + a + spare / 2.;
            list(&c.rows[..k], left, middle);
            list(&c.rows[k..], middle + 2. * GAP, right);
        }
        None => list(&c.rows, left, right),
    }
    let area = egui::Rect::from_min_max(
        pos2(right + GAP, cell.top() + 2.),
        pos2(cell.right(), cell.bottom() - 2.),
    );
    match c.part {
        Part::Frame => chart(p, area, view),
        Part::Memory => memory_chart(p, area, view),
        _ => {}
    }
}

/// A figure on a chart, on a patch of the chart's own background so lines stay behind it.
fn chart_label(p: &egui::Painter, at: egui::Pos2, align: egui::Align2, text: String) {
    let galley = p.layout_no_wrap(text, egui::FontId::monospace(9.), ink::INK_3);
    let rect = align.anchor_size(at, galley.size());
    p.rect_filled(rect.expand(1.), 1., ink::BG_WELL.gamma_multiply(0.85));
    p.galley(rect.min, galley, ink::INK_3);
}

/// Resident memory over the last two minutes, newest on the right, from zero to the top
/// of the scale. Readings further apart than a gap are not joined.
fn memory_chart(p: &egui::Painter, rect: egui::Rect, view: &View) {
    use egui::{pos2, Align2, Stroke};
    p.rect(
        rect,
        2.,
        ink::BG_WELL,
        Stroke::new(1., ink::BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(2.);
    let p = p.with_clip_rect(rect.shrink(1.));
    let top = view.memory_top.max(1.);
    let x = |age: f32| inner.right() - age / MEMORY_WINDOW_S as f32 * inner.width();
    let y = |mb: f32| inner.bottom() - (mb / top).clamp(0., 1.) * inner.height();
    p.hline(
        inner.x_range(),
        y(top / 2.).round() + 0.5,
        Stroke::new(1., ink::BORDER_SUBTLE),
    );
    let stroke = Stroke::new(1.2, ink::INK_2);
    let draw = |points: &mut Vec<egui::Pos2>| {
        match points.len() {
            0 => {}
            1 => {
                p.circle_filled(points[0], 1.2, ink::INK_2);
            }
            _ => {
                p.add(egui::Shape::line(points.clone(), stroke));
            }
        }
        points.clear();
    };
    let mut points = vec![];
    let mut previous: Option<f32> = None;
    for &(age, mb) in &view.memory {
        if previous.is_some_and(|before| before - age > MEMORY_GAP_S as f32) {
            draw(&mut points);
        }
        points.push(pos2(x(age), y(mb)));
        previous = Some(age);
    }
    draw(&mut points);
    if let Some(&(age, mb)) = view.memory.last() {
        p.circle_filled(pos2(x(age), y(mb)), 1.8, ink::INK_1);
    }
    let figure = if top.fract() == 0. {
        format!("{top:.0} MB")
    } else {
        format!("{top:.1} MB")
    };
    chart_label(&p, pos2(inner.left() + 2., inner.top() + 1.), Align2::LEFT_TOP, figure);
    chart_label(
        &p,
        pos2(inner.left() + 2., inner.bottom() - 1.),
        Align2::LEFT_BOTTOM,
        format!("{MEMORY_WINDOW_S:.0} s"),
    );
}

/// Frame intervals, oldest on the left. The line is the refresh period; a frame that took
/// one and a half of them or more is drawn in amber.
fn chart(p: &egui::Painter, rect: egui::Rect, view: &View) {
    p.rect(
        rect,
        2.,
        ink::BG_WELL,
        egui::Stroke::new(1., ink::BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(1.);
    let top = view.budget_ms * 2.5;
    let y = |ms: f32| inner.bottom() - (ms / top).min(1.) * inner.height();
    let step = inner.width() / HISTORY as f32;
    let first = HISTORY - view.intervals.len().min(HISTORY);
    for (i, ms) in view.intervals.iter().enumerate() {
        let x = inner.left() + (first + i) as f32 * step;
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, y(*ms)), egui::pos2(x + step, inner.bottom())),
            0.,
            if *ms >= view.budget_ms * DROP_AT {
                ink::ACCENT
            } else {
                ink::INK_3
            },
        );
    }
    p.hline(
        inner.x_range(),
        y(view.budget_ms),
        egui::Stroke::new(1., ink::BORDER_STRONG),
    );
    p.text(
        egui::pos2(inner.left() + 3., y(view.budget_ms) - 1.),
        egui::Align2::LEFT_BOTTOM,
        format!("{:.1} ms", view.budget_ms),
        egui::FontId::monospace(9.),
        ink::INK_3,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_take_the_fewest_rows_that_fit() {
        let five = [240., 160., 120., 200., 230.];
        let between = 25.;
        // 950 of columns and 100 between them.
        assert_eq!(pack(&five, 1050., between), vec![5]);
        assert_eq!(pack(&five, 1049., between), vec![3, 2]);
        // Three over two needs 570 for the wider row.
        assert_eq!(pack(&five, 570., between), vec![3, 2]);
        assert_eq!(pack(&five, 569., between), vec![2, 2, 1]);
        assert_eq!(pack(&five, 100., between), vec![1, 1, 1, 1, 1]);
        assert_eq!(pack(&[300., 200.], 400., between), vec![1, 1]);
        assert!(pack(&[], 400., between).is_empty());
    }

    #[test]
    fn a_row_fills_its_width_by_weight() {
        let widths = spread(&[100., 100., 100.], &[4., 1., 1.], 500., 25.);
        assert_eq!(widths, vec![200., 125., 125.]);
        assert_eq!(widths.iter().sum::<f32>() + 50., 500.);
        // Too narrow: the least widths, not squeezed.
        assert_eq!(spread(&[100., 100.], &[1., 1.], 150., 25.), vec![100., 100.]);
    }

    #[test]
    fn the_memory_scale_steps_and_holds() {
        assert_eq!(nice_ceiling(0.), 1.);
        assert_eq!(nice_ceiling(f64::NAN), 1.);
        assert_eq!(nice_ceiling(1.2), 1.5);
        assert_eq!(nice_ceiling(260.), 300.);
        assert_eq!(nice_ceiling(300.), 300.);
        assert_eq!(nice_ceiling(810.), 1000.);
        assert_eq!(nice_ceiling(1600.), 2000.);
        let mut history = MemoryHistory::default();
        for second in 0..60 {
            history.push(second as f64, 236.);
        }
        assert_eq!(history.top(), 300.);
        // A rise past the scale lifts it at once…
        history.push(60., 290.);
        assert_eq!(history.top(), 400.);
        // …and it comes down only once that reading has left the two-minute window.
        for second in 61..181 {
            history.push(second as f64, 236.);
        }
        assert_eq!(history.top(), 400.);
        history.push(181., 236.);
        assert_eq!(history.top(), 300.);
        // Old readings leave, so the history stays two minutes long.
        assert_eq!(history.samples.front().unwrap().0, 61.);
        assert!(history.samples.len() <= 122);
    }

    #[test]
    fn the_system_reports_this_process() {
        let usage = usage();
        if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            let resident = usage.resident.expect("resident memory");
            let peak = usage.peak.expect("peak resident memory");
            assert!(resident > 1 << 20, "{resident}");
            assert!(peak >= resident / 2, "{peak} {resident}");
            assert!(usage.cpu.expect("CPU time") >= 0.);
        }
    }
}
