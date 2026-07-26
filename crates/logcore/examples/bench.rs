//! 大文件基准:生成合成的 threadtime logcat,测量引擎核心热路径,
//! 用真实数字验证"10GB 文件不崩不卡",并作为后续改动的回归基线。
//!
//! 用法: cargo run --release -p logcore --example bench -- [size_gb] [file_path]
//!       [--problems-only] [--assert-contracts] [--corpus=sparse|storm]
//!       [--schedule=production|sequential]
//!   size_gb   目标大小(GiB,f64),默认 2.0
//!   file_path 合成日志路径,默认 /tmp/logfilter-bench.log
//!             若文件已存在且大小在目标 ±5% 内则复用(跳过生成)。
//!   --problems-only    只运行索引与 Problems,用于重复测量
//!   --assert-contracts 按 Problems 设计规范的硬门槛失败退出
//!   --corpus=storm     生成高密度、不同指纹的结构化事件压力语料
//!   --schedule=production 按桌面端核心交错节奏测量(默认)
//!   --schedule=sequential 先完整索引、再完整 Problems,用于冷 I/O 对照
//!
//! macOS 无 /proc,进程内不测 RSS;峰值内存请用
//!   /usr/bin/time -l cargo run --release -p logcore --example bench -- ...
//! 取输出里的 "maximum resident set size"。

use logcore::filter::{FilterField, FilterMatcher, FilterSpec, LevelMask};
use logcore::problems::DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES;
use logcore::search::{SearchMatcher, SearchSpec};
use logcore::session::{RowsView, Session};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Barrier, Mutex,
};
use std::time::{Duration, Instant};

// 架构铁律:UI 窗口只取可见窗口。基准沿用应用中的口径。
const INDEX_BUDGET: usize = 1024 * 1024; // 后台索引步进预算,与应用一致
const FILTER_CHUNK: usize = 4096; // 分块扫描步长,与应用一致
const PROBLEMS_CHUNK: usize = 4096; // Problems 每次持锁扫描上限,与应用一致
const PROBLEM_CATCH_UP_STEPS_PER_INDEX: usize = 32; // 与桌面端核心交错节奏一致
const WINDOW_ROWS: usize = 200; // UI 交互窗口一次取的行数
const WINDOW_SAMPLES: usize = 100; // 随机窗口读取采样次数
const MIN_CONCURRENT_WINDOW_SAMPLES: usize = 90;
const WINDOW_SAMPLE_MIN_INTERVAL_MS: u64 = 200;
const WINDOW_SAMPLE_JITTER_MS: usize = 201;

const MIN_PROBLEMS_LINES_PER_SECOND: f64 = 5_000_000.0;
const MAX_INDEX_AND_PROBLEMS_SECONDS: f64 = 37.0;
const MAX_PROBLEMS_WINDOW_P99_MICROS: u128 = 5_000;
const MAX_PROBLEMS_STALL_MILLIS: f64 = 20.0;
const MAX_INDEX_STALL_MILLIS: f64 = 50.0;
const MAX_PROBLEMS_RETAINED_BYTES: usize = 128 * 1024 * 1024;

const ACCEPTANCE_CORPUS_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const ACCEPTANCE_CORPUS_BYTE_TOLERANCE: u64 = ACCEPTANCE_CORPUS_BYTES / 100;
const ACCEPTANCE_MIN_LINES: usize = 70_000_000;
const ACCEPTANCE_MAX_LINES: usize = 72_500_000;
const MAX_STORED_PROBLEM_GROUPS: u32 = 100_000;
const MAX_STORED_PROBLEM_OCCURRENCES: u64 = 1_000_000;
const CORPUS_BOUNDARY_BYTES: u64 = 8 * 1024 * 1024;
const BOUNDARY_ALIGNMENT_MAX_LINE_BYTES: u64 = 4 * 1024;

// Prefix is deliberately versioned. A generic logcat buffer marker cannot prove
// that an existing file is the deterministic corpus expected by the oracles.
const SPARSE_CORPUS_HEADER: &[u8] =
    b"07-06 00:00:00.000  1 1 I LogFilterBench: format=v2 corpus=sparse\n\
--------- beginning of main\n";
const STORM_CORPUS_HEADER: &[u8] =
    b"07-06 00:00:00.000  1 1 I LogFilterBench: format=v2 corpus=storm\n\
--------- beginning of events\n";
const CORPUS_HEADER_LINES: usize = 2;

const MIB: f64 = 1024.0 * 1024.0;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// 约 40 个真实 logcat tag(不含冒号,避免破坏 tag:message 切分)。
const TAGS: &[&str] = &[
    "ActivityManager",
    "SurfaceFlinger",
    "NetPolicy",
    "chatty",
    "WifiService",
    "AudioTrack",
    "PowerManagerService",
    "WindowManager",
    "PackageManager",
    "InputDispatcher",
    "ConnectivityService",
    "BatteryService",
    "LocationManager",
    "SensorService",
    "GraphicsStats",
    "art",
    "dalvikvm",
    "System.err",
    "libEGL",
    "OpenGLRenderer",
    "Choreographer",
    "ViewRootImpl",
    "BluetoothAdapter",
    "TelephonyManager",
    "MediaPlayer",
    "CameraService",
    "NotificationService",
    "AlarmManager",
    "JobScheduler",
    "DownloadManager",
    "AccountManager",
    "ContentResolver",
    "KeyguardService",
    "StatusBar",
    "Launcher",
    "GmsCore",
    "Zygote",
    "installd",
    "netd",
    "vold",
];

/// 消息模板碎片,拼接出可被 word 过滤/搜索/正则命中的语料。
const VERBS: &[&str] = &[
    "starting",
    "stopping",
    "binding",
    "unbinding",
    "scheduling",
    "dispatching",
    "handling",
    "flushing",
    "committing",
    "aborting",
    "retrying",
    "resuming",
];
const NOUNS: &[&str] = &[
    "connection",
    "transaction",
    "activity",
    "service",
    "broadcast",
    "socket",
    "buffer",
    "surface",
    "session",
    "request",
    "packet",
    "frame",
];
/// 含 "conn" 前缀 + "timeout"/"reset" 尾巴,专供正则 `conn.*(timeout|reset)` 命中。
const CONN_EVENTS: &[&str] = &[
    "conn attempt failed timeout",
    "connection lost reset by peer",
    "conn pool reset after idle",
    "connect handshake timeout exceeded",
];

/// 极简 xorshift64* PRNG:std-only,无需 rand 依赖。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// 返回 [0, n) 的伪随机值。
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// 一次基准运行收集的所有指标,末尾对齐打印。
struct Metrics {
    rows: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorpusKind {
    Sparse,
    Storm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchSchedule {
    Production,
    Sequential,
}

impl BenchSchedule {
    fn parse(value: &str) -> Self {
        match value {
            "production" => Self::Production,
            "sequential" => Self::Sequential,
            _ => fail("--schedule 必须是 production 或 sequential"),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Sequential => "sequential",
        }
    }
}

impl CorpusKind {
    fn parse(value: &str) -> Self {
        match value {
            "sparse" => Self::Sparse,
            "storm" => Self::Storm,
            _ => fail("--corpus 必须是 sparse 或 storm"),
        }
    }

    const fn header(self) -> &'static [u8] {
        match self {
            Self::Sparse => SPARSE_CORPUS_HEADER,
            Self::Storm => STORM_CORPUS_HEADER,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Storm => "storm",
        }
    }
}

#[derive(Debug)]
struct BenchOptions {
    size_gb: f64,
    path: PathBuf,
    problems_only: bool,
    assert_contracts: bool,
    corpus: CorpusKind,
    schedule: BenchSchedule,
}

#[derive(Clone, Copy, Debug)]
struct IndexBenchResult {
    elapsed: Duration,
    max_stall: Duration,
}

#[derive(Clone, Copy, Debug)]
struct ProblemsBenchResult {
    standalone_lines_per_second: Option<f64>,
    combined: Duration,
    max_stall: Duration,
    window_p99_micros: Option<u128>,
    concurrent_window_samples: usize,
    indexing_window_samples: usize,
    catching_up_window_samples: usize,
    observed_occurrence_count: u64,
    stored_occurrence_count: u64,
    stored_group_count: u32,
    stats_limited: bool,
    memory_high_water_bytes: usize,
    memory_retained_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum BenchPhase {
    Initializing = 0,
    Indexing = 1,
    CatchingUpProblems = 2,
    FinishPending = 3,
    Done = 4,
}

impl BenchPhase {
    fn load(value: &AtomicUsize) -> Self {
        match value.load(Ordering::Acquire) {
            1 => Self::Indexing,
            2 => Self::CatchingUpProblems,
            3 => Self::FinishPending,
            4 => Self::Done,
            _ => Self::Initializing,
        }
    }

    fn store(self, value: &AtomicUsize) {
        value.store(self as usize, Ordering::Release);
    }
}

#[derive(Debug, Default)]
struct WindowLatencySamples {
    indexing: Vec<u128>,
    catching_up: Vec<u128>,
    finish_pending: Vec<u128>,
}

impl WindowLatencySamples {
    fn record(&mut self, phase: BenchPhase, micros: u128) {
        match phase {
            BenchPhase::Indexing => self.indexing.push(micros),
            BenchPhase::CatchingUpProblems => self.catching_up.push(micros),
            BenchPhase::FinishPending => self.finish_pending.push(micros),
            BenchPhase::Initializing | BenchPhase::Done => {}
        }
    }

    fn extend(&mut self, other: Self) {
        self.indexing.extend(other.indexing);
        self.catching_up.extend(other.catching_up);
        self.finish_pending.extend(other.finish_pending);
    }

    fn active_count(&self) -> usize {
        self.indexing
            .len()
            .saturating_add(self.catching_up.len())
            .saturating_add(self.finish_pending.len())
    }

    fn active_summary(&self) -> Option<LatencySummary> {
        let mut samples = Vec::with_capacity(self.active_count());
        samples.extend_from_slice(&self.indexing);
        samples.extend_from_slice(&self.catching_up);
        samples.extend_from_slice(&self.finish_pending);
        LatencySummary::from_samples(samples)
    }
}

#[derive(Clone, Copy, Debug)]
struct LatencySummary {
    min: u128,
    median: u128,
    p99: u128,
    max: u128,
}

impl LatencySummary {
    fn from_samples(samples: Vec<u128>) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let (min, median, p99, max) = latency_stats(samples);
        Some(Self {
            min,
            median,
            p99,
            max,
        })
    }
}

impl Metrics {
    fn new() -> Self {
        Metrics { rows: Vec::new() }
    }

    fn push(&mut self, label: impl Into<String>, value: impl Into<String>) {
        self.rows.push((label.into(), value.into()));
    }

    fn print(&self) {
        let width = self.rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        println!("\n==================== 基准汇总 ====================");
        for (label, value) in &self.rows {
            println!("  {label:<width$}  {value}");
        }
        println!("=================================================");
    }
}

fn main() {
    let options = parse_options();
    if options.size_gb <= 0.0 {
        fail("size_gb 必须为正数");
    }

    let target_bytes = (options.size_gb * GIB) as u64;
    println!(
        "目标大小 {:.2} GiB (~{target_bytes} bytes)  corpus={}  schedule={}  路径 {}",
        options.size_gb,
        options.corpus.label(),
        options.schedule.label(),
        options.path.display()
    );

    let mut metrics = Metrics::new();
    metrics.push("目标大小 (GiB)", format!("{:.2}", options.size_gb));
    metrics.push("Problems corpus", options.corpus.label());
    metrics.push("调度口径", options.schedule.label());

    // Phase G:生成(可复用则跳过)。
    let actual_bytes = ensure_corpus(&options.path, target_bytes, options.corpus, &mut metrics);
    metrics.push(
        "文件字节数",
        format!("{actual_bytes} ({:.2} GiB)", actual_bytes as f64 / GIB),
    );

    // Phase 1/2:按选择的调度口径执行索引与 Problems。
    let mut session =
        Session::open(&options.path).unwrap_or_else(|e| fail(&format!("打开失败: {e}")));
    let (total_lines, index_result, problems_result) = match options.schedule {
        BenchSchedule::Production => phase_interleaved(&mut session, actual_bytes, &mut metrics),
        BenchSchedule::Sequential => {
            let (total_lines, index_result, index_window_samples) =
                phase_index(&mut session, actual_bytes, &mut metrics);
            let problems_result = phase_problems(
                &mut session,
                total_lines,
                index_result.elapsed,
                index_window_samples,
                &mut metrics,
            );
            (total_lines, index_result, problems_result)
        }
    };

    if !options.problems_only {
        // Phase 3:过滤。
        let filter_2b = phase_filters(&session, total_lines, &mut metrics);

        // Phase 4:窗口读取(All + Filtered)。
        phase_window_reads(&mut session, total_lines, &filter_2b, &mut metrics);

        // Phase 5:搜索。
        phase_search(&session, total_lines, &mut metrics);

        // Phase 6:导出(Filtered 用 3b 过滤,All 全量)。
        phase_export(&mut session, &options.path, &mut metrics);
    }

    metrics.print();
    if options.assert_contracts {
        let complete = assert_problem_contracts(
            &options,
            actual_bytes,
            total_lines,
            index_result,
            problems_result,
        );
        if complete {
            println!("\n本次调度适用的 Problems 硬门槛: PASS");
        } else {
            println!(
                "\nproduction 调度硬门槛(不含 Problems standalone ≥5M): PASS\n\
                 standalone 门槛必须另跑 --schedule=sequential。"
            );
        }
        println!("注意:这只是单次测量,不能替代同机同 corpus 的冷/暖缓存各 3 次及其中位数。");
    }
    println!(
        "\n提示: macOS 无 /proc,进程内不测 RSS。峰值内存请用\n  \
         /usr/bin/time -l cargo run --release -p logcore --example bench -- {:.2} {}\n  \
         读取输出中的 \"maximum resident set size\"。",
        options.size_gb,
        options.path.display()
    );
}

fn parse_options() -> BenchOptions {
    let mut positionals = Vec::new();
    let mut problems_only = false;
    let mut assert_contracts = false;
    let mut corpus = CorpusKind::Sparse;
    let mut schedule = BenchSchedule::Production;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--problems-only" => problems_only = true,
            "--assert-contracts" => assert_contracts = true,
            _ if argument.starts_with("--corpus=") => {
                corpus = CorpusKind::parse(
                    argument
                        .strip_prefix("--corpus=")
                        .expect("prefix checked above"),
                );
            }
            _ if argument.starts_with("--schedule=") => {
                schedule = BenchSchedule::parse(
                    argument
                        .strip_prefix("--schedule=")
                        .expect("prefix checked above"),
                );
            }
            _ if argument.starts_with("--") => fail(&format!("未知参数: {argument}")),
            _ => positionals.push(argument),
        }
    }
    if positionals.len() > 2 {
        fail("位置参数最多为 size_gb 与 file_path");
    }
    let size_gb = positionals
        .first()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| fail("size_gb 必须是数字,例如 2.0"))
        })
        .unwrap_or(2.0);
    let path = positionals
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/logfilter-bench.log"));
    BenchOptions {
        size_gb,
        path,
        problems_only,
        assert_contracts,
        corpus,
        schedule,
    }
}

/// 保证语料存在:大小在目标 ±5% 内则复用,否则重新生成。返回实际字节数。
fn ensure_corpus(path: &Path, target_bytes: u64, corpus: CorpusKind, metrics: &mut Metrics) -> u64 {
    if let Ok(meta) = fs::metadata(path) {
        let existing = meta.len();
        let low = target_bytes as f64 * 0.95;
        let high = target_bytes as f64 * 1.05;
        if (existing as f64) >= low
            && (existing as f64) <= high
            && corpus_header_matches(path, corpus)
        {
            println!(
                "复用已存在文件: {existing} bytes ({:.2} GiB) 在目标 ±5% 内,跳过生成。",
                existing as f64 / GIB
            );
            metrics.push("生成", "复用(跳过)".to_string());
            return existing;
        }
        println!("已存在文件大小或 corpus 标识不匹配,重新生成({existing} bytes)。");
    }
    generate(path, target_bytes, corpus, metrics)
}

fn corpus_header_matches(path: &Path, corpus: CorpusKind) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let mut header = vec![0; corpus.header().len()];
    reader.read_exact(&mut header).is_ok() && header == corpus.header()
}

/// Phase G:写 threadtime 行,BufWriter 8MB 缓冲。返回写出的字节数。
fn generate(path: &Path, target_bytes: u64, corpus: CorpusKind, metrics: &mut Metrics) -> u64 {
    println!("生成中 ... (每 512MB 报告一次进度)");
    let file = fs::File::create(path).unwrap_or_else(|e| fail(&format!("创建文件失败: {e}")));
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
    let mut rng = Rng::new(0x9E3779B97F4A7C15);

    // pid/tid 池:约 30 个 pid。
    let pids: Vec<u32> = (0..30).map(|i| 1000 + i * 137).collect();

    writer
        .write_all(corpus.header())
        .unwrap_or_else(|error| fail(&format!("写 corpus 标识失败: {error}")));
    let mut written = corpus.header().len() as u64;
    let mut line_no: u64 = 0;
    let mut next_progress: u64 = 512 * 1024 * 1024;

    // 时间戳单调推进,从固定起点起,每行涨 1~9 毫秒。日期随跨日推进,
    // 避免 10GiB corpus 每 24 小时制造一次非预期的时间回拨。
    let mut elapsed_ms: u64 = 0;
    let mut msg = String::with_capacity(1200);
    let boundary_problem_first_line_bytes = formatted_threadtime_line_len(
        7,
        6,
        0,
        0,
        0,
        0,
        4242,
        4242,
        'E',
        "AndroidRuntime",
        "FATAL EXCEPTION: main",
    ) as u64;
    let mut boundary_problem_part = None;
    let mut boundary_problem_inserted = false;
    let t0 = Instant::now();

    let mut result: io::Result<()> = Ok(());
    while written < target_bytes || boundary_problem_part.is_some() {
        line_no += 1;
        elapsed_ms += 1 + (rng.next_u64() % 9);
        let ms = elapsed_ms % 1000;
        let total_s = elapsed_ms / 1000;
        let day_offset = total_s / 86_400;
        let (month, day) = month_day_from_july_six(day_offset);
        let hh = (total_s / 3600) % 24;
        let mm = (total_s / 60) % 60;
        let ss = total_s % 60;

        // 稀疏语料每约 100 万行插入一条原始分隔行(不符合 threadtime,
        // 考验解析健壮性)。事件风暴保持 Events provenance。
        if corpus == CorpusKind::Sparse && line_no.is_multiple_of(1_000_000) {
            if let Err(e) = writer.write_all(b"--------- beginning of main\n") {
                result = Err(e);
                break;
            }
            written += 28;
            continue;
        }

        let (pid, tid, level, tag) = match corpus {
            CorpusKind::Storm => build_storm_problem_line(line_no, &mut msg),
            CorpusKind::Sparse => {
                if let Some(part) = boundary_problem_part {
                    let problem_line = build_sparse_problem_part(part, &mut msg);
                    boundary_problem_part = (part + 1 < 4).then_some(part + 1);
                    problem_line
                } else if !boundary_problem_inserted {
                    if let Some(alignment_line) = build_boundary_alignment_line(
                        written,
                        boundary_problem_first_line_bytes,
                        month,
                        day,
                        hh,
                        mm,
                        ss,
                        ms,
                        &mut msg,
                    ) {
                        // The next crash starts before byte 8MiB and its remaining
                        // rows continue after that index slice.
                        boundary_problem_part = Some(0);
                        boundary_problem_inserted = true;
                        alignment_line
                    } else if let Some(problem_line) = build_sparse_problem_line(line_no, &mut msg)
                    {
                        problem_line
                    } else {
                        let pid = pids[rng.below(pids.len())];
                        let tid = pid + (rng.next_u64() % 8) as u32; // tid 贴近 pid
                        let level = pick_level(&mut rng);
                        let tag = TAGS[rng.below(TAGS.len())];
                        build_message(&mut rng, line_no, &mut msg);
                        (pid, tid, level, tag)
                    }
                } else if let Some(problem_line) = build_sparse_problem_line(line_no, &mut msg) {
                    problem_line
                } else {
                    let pid = pids[rng.below(pids.len())];
                    let tid = pid + (rng.next_u64() % 8) as u32; // tid 贴近 pid
                    let level = pick_level(&mut rng);
                    let tag = TAGS[rng.below(TAGS.len())];
                    build_message(&mut rng, line_no, &mut msg);
                    (pid, tid, level, tag)
                }
            }
        };

        // 手写格式化直接写进 BufWriter,减少中间 String 分配。
        let line = format!(
            "{month:02}-{day:02} {hh:02}:{mm:02}:{ss:02}.{ms:03}  {pid} {tid} {level} {tag}: {msg}\n"
        );
        if let Err(e) = writer.write_all(line.as_bytes()) {
            result = Err(e);
            break;
        }
        written += line.len() as u64;

        if written >= next_progress {
            println!(
                "  已写 {:.2} GiB / {:.2} GiB ({} 行)",
                written as f64 / GIB,
                target_bytes as f64 / GIB,
                line_no
            );
            next_progress += 512 * 1024 * 1024;
        }
    }

    if result.is_err() || writer.flush().is_err() {
        // 磁盘写入失败(如 ENOSPC)明确报错中止。
        let msg = result
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "flush 失败".to_string());
        fail(&format!(
            "写盘失败(可能磁盘空间不足 ENOSPC): {msg}。已写 {written} bytes,请清理磁盘或换用更小的 size_gb。"
        ));
    }

    let elapsed = t0.elapsed();
    let mb_s = (written as f64 / MIB) / elapsed.as_secs_f64();
    println!(
        "生成完成: {written} bytes ({:.2} GiB), {line_no} 行, {:.1}s, {mb_s:.0} MB/s",
        written as f64 / GIB,
        elapsed.as_secs_f64()
    );
    metrics.push("生成吞吐 (MB/s)", format!("{mb_s:.0}"));
    metrics.push("生成耗时 (s)", format!("{:.1}", elapsed.as_secs_f64()));
    written
}

fn month_day_from_july_six(day_offset: u64) -> (u32, u32) {
    const MONTH_DAYS: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month_index = 6usize;
    let mut zero_based_day = 5u64.saturating_add(day_offset);
    loop {
        let days = MONTH_DAYS[month_index];
        if zero_based_day < days {
            return ((month_index + 1) as u32, (zero_based_day + 1) as u32);
        }
        zero_based_day -= days;
        month_index = (month_index + 1) % MONTH_DAYS.len();
    }
}

/// 每约 100 万行插入一个确定性的四行 Java crash。它让 10GiB 语料保持
/// 稀疏事件密度,同时覆盖多行状态机和高重复 fingerprint;原始正文仍只存在
/// 于源文件,Problems 索引只保存紧凑引用。
fn build_sparse_problem_line(
    line_no: u64,
    out: &mut String,
) -> Option<(u32, u32, char, &'static str)> {
    let part = match line_no % 1_000_000 {
        10 => 0,
        11 => 1,
        12 => 2,
        13 => 3,
        _ => return None,
    };
    Some(build_sparse_problem_part(part, out))
}

fn build_sparse_problem_part(part: usize, out: &mut String) -> (u32, u32, char, &'static str) {
    let pid = 4242;
    let message = match part {
        0 => "FATAL EXCEPTION: main",
        1 => "Process: com.example.synthetic, PID: 4242",
        2 => "java.lang.IllegalStateException: synthetic benchmark crash 123",
        3 => "    at com.example.synthetic.MainKt.run(Main.kt:42)",
        _ => unreachable!("sparse problem part is bounded to four rows"),
    };
    out.clear();
    out.push_str(message);
    (pid, pid, 'E', "AndroidRuntime")
}

#[allow(clippy::too_many_arguments)]
fn build_boundary_alignment_line(
    written: u64,
    first_problem_line_bytes: u64,
    month: u32,
    day: u32,
    hh: u64,
    mm: u64,
    ss: u64,
    ms: u64,
    out: &mut String,
) -> Option<(u32, u32, char, &'static str)> {
    let boundary = CORPUS_BOUNDARY_BYTES;
    let target_line_bytes = boundary
        .checked_sub(written)?
        .checked_sub(first_problem_line_bytes)?;
    if target_line_bytes > BOUNDARY_ALIGNMENT_MAX_LINE_BYTES {
        return None;
    }

    let empty_line_bytes = formatted_threadtime_line_len(
        month,
        day,
        hh,
        mm,
        ss,
        ms,
        4242,
        4242,
        'I',
        "LogFilterBench",
        "",
    ) as u64;
    let message_bytes = target_line_bytes.checked_sub(empty_line_bytes)?;
    const PREFIX: &str = "boundary-align ";
    if message_bytes < PREFIX.len() as u64 {
        return None;
    }
    out.clear();
    out.push_str(PREFIX);
    out.extend(std::iter::repeat_n(
        'x',
        (message_bytes as usize).saturating_sub(PREFIX.len()),
    ));
    Some((4242, 4242, 'I', "LogFilterBench"))
}

#[allow(clippy::too_many_arguments)]
fn formatted_threadtime_line_len(
    month: u32,
    day: u32,
    hh: u64,
    mm: u64,
    ss: u64,
    ms: u64,
    pid: u32,
    tid: u32,
    level: char,
    tag: &str,
    message: &str,
) -> usize {
    format!(
        "{month:02}-{day:02} {hh:02}:{mm:02}:{ss:02}.{ms:03}  \
         {pid} {tid} {level} {tag}: {message}\n"
    )
    .len()
}

/// 每一行都是合法的 events-buffer am_crash,且 process/signature 随行号变化。
/// 该语料用于尽快打满 group/event/intern 上限,验证内存预算与 limited 语义,
/// 不代表真实 Android 日志中的事件密度。
fn build_storm_problem_line(line_no: u64, out: &mut String) -> (u32, u32, char, &'static str) {
    let pid = 10_000 + (line_no % 50_000) as u32;
    out.clear();
    write!(
        out,
        "[{pid},com.example.storm.{line_no},0,java.lang.RuntimeException,synthetic-{line_no},Main.kt,{}]",
        line_no % 10_000
    )
    .expect("writing to a String cannot fail");
    (pid, pid, 'I', "am_crash")
}

/// 按 V10% D40% I30% W12% E7% F1% 分布抽级别。
fn pick_level(rng: &mut Rng) -> char {
    match rng.below(100) {
        0..=9 => 'V',
        10..=49 => 'D',
        50..=79 => 'I',
        80..=91 => 'W',
        92..=98 => 'E',
        _ => 'F',
    }
}

/// 构造消息:普通行 40~160 字符,每约 1 万行来一条 ~1KB 长行。
fn build_message(rng: &mut Rng, line_no: u64, out: &mut String) {
    out.clear();
    // 约 1/6 的行嵌入 conn 事件,供正则 conn.*(timeout|reset) 命中。
    if rng.below(6) == 0 {
        out.push_str(CONN_EVENTS[rng.below(CONN_EVENTS.len())]);
        out.push(' ');
    }
    let verb = VERBS[rng.below(VERBS.len())];
    let noun = NOUNS[rng.below(NOUNS.len())];
    out.push_str(verb);
    out.push(' ');
    out.push_str(noun);
    out.push_str(" id=");
    out.push_str(&line_no.to_string());

    if line_no.is_multiple_of(10_000) {
        // 长行:填充到 ~1KB。
        while out.len() < 1024 {
            out.push_str(" pad=0123456789abcdef");
        }
        return;
    }

    // 目标长度 40~160。
    let target = 40 + rng.below(121);
    while out.len() < target {
        out.push(' ');
        out.push_str(NOUNS[rng.below(NOUNS.len())]);
    }
}

fn sample_windows_until_done(
    shared: &Mutex<&mut Session>,
    done: &AtomicBool,
    phase: &AtomicUsize,
    seed: u64,
    max_samples: usize,
) -> WindowLatencySamples {
    let mut samples = WindowLatencySamples::default();
    let mut rng = Rng::new(seed);
    let mut next_sample = Instant::now();
    while samples.active_count() < max_samples && !done.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next_sample {
            std::thread::sleep(
                next_sample
                    .duration_since(now)
                    .min(Duration::from_millis(1)),
            );
            continue;
        }

        // Classify by the phase active when the request was issued. If that phase
        // completes while this call waits, the latency still belongs to the work
        // that occupied the Session lock at request time.
        let request_phase = BenchPhase::load(phase);
        let sample_started = Instant::now();
        let rows = {
            let guard = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stable = guard.stable_lines();
            let span = stable.saturating_sub(WINDOW_ROWS).max(1);
            let start = rng.below(span);
            guard.get_rows_for_view(RowsView::All, start, WINDOW_ROWS)
        };
        let elapsed = sample_started.elapsed().as_micros();
        std::hint::black_box(rows.len());
        samples.record(request_phase, elapsed);
        next_sample = Instant::now()
            + Duration::from_millis(
                WINDOW_SAMPLE_MIN_INTERVAL_MS
                    + u64::try_from(rng.below(WINDOW_SAMPLE_JITTER_MS)).unwrap_or(0),
            );
    }
    samples
}

fn format_latency_summary(summary: Option<LatencySummary>) -> String {
    match summary {
        Some(summary) => format!(
            "min {}µs / median {}µs / p99 {}µs / max {}µs",
            summary.min, summary.median, summary.p99, summary.max
        ),
        None => "无扫描期样本".to_string(),
    }
}

fn print_window_phase_coverage(samples: &WindowLatencySamples) {
    println!(
        "  窗口 phase 覆盖: Indexing {} / CatchingUpProblems {} / FinishPending {}",
        samples.indexing.len(),
        samples.catching_up.len(),
        samples.finish_pending.len()
    );
    if samples.finish_pending.is_empty() {
        println!("  FinishPending 短于本次有界周期采样窗口,没有命中;不以结束后空闲读取补样。");
    }
}

/// 模拟桌面端静态文件 worker 的核心交错节奏：每个 8MiB index 临界区后,
/// 用最多 32 个独立 4096-line Problems 临界区追赶刚发布的稳定前沿。
///
/// 这里不包含 Tauri generation 校验、状态 DTO 和事件发送开销，因此锁计时仅代表
/// logcore 核心调用。读取线程贯穿 Indexing、Problems catch-up 与最终 finish,
/// 不向 worker 发优先级信号，所有样本都是真实的 Session 锁竞争。
fn phase_interleaved(
    session: &mut Session,
    total_bytes: u64,
    metrics: &mut Metrics,
) -> (usize, IndexBenchResult, ProblemsBenchResult) {
    println!(
        "\n[Phase 1+2] 生产交错调度 (index {INDEX_BUDGET} bytes; \
         Problems {PROBLEMS_CHUNK} 行 × 最多 {PROBLEM_CATCH_UP_STEPS_PER_INDEX}/slice) ..."
    );
    let shared = Mutex::new(session);
    let done = AtomicBool::new(false);
    let phase = AtomicUsize::new(BenchPhase::Initializing as usize);
    let start = Barrier::new(2);

    let (
        total_lines,
        index_duration,
        index_max_stall,
        index_steps,
        problems_duration,
        problems_max_stall,
        problem_steps,
        combined,
        window_samples,
    ) = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            BenchPhase::Indexing.store(&phase);
            start.wait();
            let combined_started = Instant::now();
            let mut index_duration = Duration::ZERO;
            let mut index_max_stall = Duration::ZERO;
            let mut index_steps = 0u64;
            let mut problems_duration = Duration::ZERO;
            let mut problems_max_stall = Duration::ZERO;
            let mut problem_steps = 0u64;

            let total_lines = loop {
                BenchPhase::Indexing.store(&phase);
                let index_done = {
                    let mut guard = shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let started = Instant::now();
                    let index_done = guard.index_step(INDEX_BUDGET);
                    drop(guard);
                    let elapsed = started.elapsed();
                    index_duration += elapsed;
                    index_max_stall = index_max_stall.max(elapsed);
                    index_steps += 1;
                    index_done
                };

                let step_limit = if index_done {
                    usize::MAX
                } else {
                    PROBLEM_CATCH_UP_STEPS_PER_INDEX
                };
                let mut caught_up = false;
                BenchPhase::CatchingUpProblems.store(&phase);
                for _ in 0..step_limit {
                    let step = {
                        let mut guard = shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let started = Instant::now();
                        let step = guard.scan_problems_step(PROBLEMS_CHUNK);
                        drop(guard);
                        let elapsed = started.elapsed();
                        problems_duration += elapsed;
                        problems_max_stall = problems_max_stall.max(elapsed);
                        problem_steps += 1;
                        step
                    };
                    caught_up = step.caught_up;
                    if caught_up {
                        break;
                    }
                    std::thread::yield_now();
                }

                if index_done && caught_up {
                    BenchPhase::FinishPending.store(&phase);
                    let total_lines = {
                        let mut guard = shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let started = Instant::now();
                        let finish = guard.finish_problem_input();
                        let total_lines = guard.total_lines();
                        drop(guard);
                        let elapsed = started.elapsed();
                        problems_duration += elapsed;
                        problems_max_stall = problems_max_stall.max(elapsed);
                        problem_steps += 1;
                        if !finish.finished {
                            fail("Problems 未在最终稳定前沿完成封口");
                        }
                        total_lines
                    };
                    break total_lines;
                }
                std::thread::yield_now();
            };
            BenchPhase::Done.store(&phase);
            done.store(true, Ordering::Release);
            (
                total_lines,
                index_duration,
                index_max_stall,
                index_steps,
                problems_duration,
                problems_max_stall,
                problem_steps,
                combined_started.elapsed(),
            )
        });

        start.wait();
        let window_samples =
            sample_windows_until_done(&shared, &done, &phase, 0xA0761D6478BD642F, WINDOW_SAMPLES);
        let (
            total_lines,
            index_duration,
            index_max_stall,
            index_steps,
            problems_duration,
            problems_max_stall,
            problem_steps,
            combined,
        ) = worker.join().expect("production benchmark worker panicked");

        (
            total_lines,
            index_duration,
            index_max_stall,
            index_steps,
            problems_duration,
            problems_max_stall,
            problem_steps,
            combined,
            window_samples,
        )
    });

    let window_summary = window_samples.active_summary();
    let concurrent_samples = window_samples.active_count();
    let p99 = window_summary.map(|summary| summary.p99);
    let index_mb_s = (total_bytes as f64 / MIB) / index_duration.as_secs_f64();
    let index_lines_s = total_lines as f64 / index_duration.as_secs_f64();
    let problems_lines_s = total_lines as f64 / problems_duration.as_secs_f64();
    let index_avg_stall_ms = index_duration.as_secs_f64() * 1_000.0 / index_steps.max(1) as f64;
    let problems_avg_stall_ms =
        problems_duration.as_secs_f64() * 1_000.0 / problem_steps.max(1) as f64;
    let stats = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .problem_stats();
    let memory = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .problem_memory_stats();

    println!(
        "  索引: {total_lines} 行, {:.2}s, {index_mb_s:.0} MB/s, \
         单步 avg {index_avg_stall_ms:.2}ms / max {:.2}ms",
        index_duration.as_secs_f64(),
        index_max_stall.as_secs_f64() * 1_000.0
    );
    println!(
        "  Problems CPU/热页: {:.2}s, {:.1} M行/s, \
         单步 avg {problems_avg_stall_ms:.2}ms / max {:.2}ms",
        problems_duration.as_secs_f64(),
        problems_lines_s / 1_000_000.0,
        problems_max_stall.as_secs_f64() * 1_000.0
    );
    println!(
        "  index + Problems 生产墙钟: {:.2}s; 扫描期窗口 {}",
        combined.as_secs_f64(),
        format_latency_summary(window_summary)
    );
    print_window_phase_coverage(&window_samples);
    println!(
        "  事件: 检出 {} / 保存 {} / 分组 {} / limited={}; \
         内存 retained {:.2}MiB / high-water {:.2}MiB",
        stats.observed_occurrence_count,
        stats.stored_occurrence_count,
        stats.stored_group_count,
        stats.limited,
        memory.retained_heap_bytes as f64 / MIB,
        memory.high_water_charged_bytes as f64 / MIB
    );

    metrics.push("总行数", total_lines.to_string());
    metrics.push(
        "索引耗时 (s)",
        format!("{:.2}", index_duration.as_secs_f64()),
    );
    metrics.push("索引吞吐 (MB/s)", format!("{index_mb_s:.0}"));
    metrics.push("索引 (行/s)", format!("{index_lines_s:.0}"));
    metrics.push("索引单步 avg (ms)", format!("{index_avg_stall_ms:.2}"));
    metrics.push(
        "索引单步 max (ms)",
        format!("{:.2}", index_max_stall.as_secs_f64() * 1_000.0),
    );
    metrics.push(
        "Problems core-call CPU/热页 (M行/s)",
        format!("{:.1}", problems_lines_s / 1_000_000.0),
    );
    metrics.push(
        "Problems CPU/热页耗时 (s)",
        format!("{:.2}", problems_duration.as_secs_f64()),
    );
    metrics.push(
        "index + Problems (s)",
        format!("{:.2}", combined.as_secs_f64()),
    );
    metrics.push(
        "Problems 单步 max (ms)",
        format!("{:.2}", problems_max_stall.as_secs_f64() * 1_000.0),
    );
    metrics.push(
        "生产调度窗口 (µs)",
        format!(
            "{}; phase index {} / catching-up {} / finish {}",
            format_latency_summary(window_summary),
            window_samples.indexing.len(),
            window_samples.catching_up.len(),
            window_samples.finish_pending.len()
        ),
    );
    metrics.push(
        "Problems 事件",
        format!(
            "observed {} / stored {} / groups {} / limited {}",
            stats.observed_occurrence_count,
            stats.stored_occurrence_count,
            stats.stored_group_count,
            stats.limited
        ),
    );
    metrics.push(
        "Problems 受控内存 (MiB)",
        format!(
            "charged {:.2} / retained {:.2} / high-water {:.2} / limit {:.2} / denied {}",
            memory.charged_bytes as f64 / MIB,
            memory.retained_heap_bytes as f64 / MIB,
            memory.high_water_charged_bytes as f64 / MIB,
            memory.limit_bytes as f64 / MIB,
            memory.denied_reservation_count
        ),
    );

    (
        total_lines,
        IndexBenchResult {
            elapsed: index_duration,
            max_stall: index_max_stall,
        },
        ProblemsBenchResult {
            // Interleaved core-call time is useful CPU/hot-page diagnosis, but it
            // is not the spec's standalone wall-clock throughput.
            standalone_lines_per_second: None,
            combined,
            max_stall: problems_max_stall,
            window_p99_micros: p99,
            concurrent_window_samples: concurrent_samples,
            indexing_window_samples: window_samples.indexing.len(),
            catching_up_window_samples: window_samples.catching_up.len(),
            observed_occurrence_count: stats.observed_occurrence_count,
            stored_occurrence_count: stats.stored_occurrence_count,
            stored_group_count: stats.stored_group_count,
            stats_limited: stats.limited,
            memory_high_water_bytes: memory.high_water_charged_bytes,
            memory_retained_bytes: memory.retained_heap_bytes,
        },
    )
}

/// Phase 1:打开后按 8MiB 预算步进索引至完成。返回总行数与耗时。
fn phase_index(
    session: &mut Session,
    total_bytes: u64,
    metrics: &mut Metrics,
) -> (usize, IndexBenchResult, WindowLatencySamples) {
    println!("\n[Phase 1] 索引 (每 1GB 报告进度,并发真实争锁读取窗口) ...");
    let shared = Mutex::new(session);
    let done = AtomicBool::new(false);
    let phase = AtomicUsize::new(BenchPhase::Initializing as usize);
    let start = Barrier::new(2);
    let (total_lines, elapsed, max_stall, sum_stall, steps, window_samples) =
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                BenchPhase::Indexing.store(&phase);
                start.wait();
                let t0 = Instant::now();
                let mut steps: u64 = 0;
                let mut max_stall = Duration::ZERO;
                let mut sum_stall = Duration::ZERO;
                let mut next_progress: usize = 1024 * 1024 * 1024;
                let total_lines = loop {
                    let (index_done, indexed, total_lines, stall) = {
                        let mut guard = shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let step_start = Instant::now();
                        let index_done = guard.index_step(INDEX_BUDGET);
                        let indexed = guard.indexed_bytes();
                        let total_lines = guard.total_lines();
                        drop(guard);
                        let stall = step_start.elapsed();
                        (index_done, indexed, total_lines, stall)
                    };
                    steps += 1;
                    sum_stall += stall;
                    max_stall = max_stall.max(stall);
                    if indexed >= next_progress {
                        println!(
                            "  已索引 {:.2} GiB / {:.2} GiB ({} 行)",
                            indexed as f64 / GIB,
                            total_bytes as f64 / GIB,
                            total_lines
                        );
                        next_progress += 1024 * 1024 * 1024;
                    }
                    if index_done {
                        break total_lines;
                    }
                    std::thread::yield_now();
                };
                let elapsed = t0.elapsed();
                BenchPhase::Done.store(&phase);
                done.store(true, Ordering::Release);
                (total_lines, elapsed, max_stall, sum_stall, steps)
            });
            start.wait();
            let window_samples = sample_windows_until_done(
                &shared,
                &done,
                &phase,
                0x94D049BB133111EB,
                WINDOW_SAMPLES / 2,
            );
            let (total_lines, elapsed, max_stall, sum_stall, steps) =
                worker.join().expect("index benchmark worker panicked");
            (
                total_lines,
                elapsed,
                max_stall,
                sum_stall,
                steps,
                window_samples,
            )
        });

    let mb_s = (total_bytes as f64 / MIB) / elapsed.as_secs_f64();
    let lines_s = total_lines as f64 / elapsed.as_secs_f64();
    let avg_stall_ms = sum_stall.as_secs_f64() * 1000.0 / steps as f64;
    let max_stall_ms = max_stall.as_secs_f64() * 1000.0;

    println!(
        "  完成: {} 行, {:.1}s, {mb_s:.0} MB/s, {lines_s:.0} 行/s",
        total_lines,
        elapsed.as_secs_f64()
    );
    println!("  单步停顿(近似会话锁持有): 平均 {avg_stall_ms:.2}ms, 最大 {max_stall_ms:.2}ms ({steps} 步)");

    metrics.push("总行数", total_lines.to_string());
    metrics.push("索引耗时 (s)", format!("{:.1}", elapsed.as_secs_f64()));
    metrics.push("索引吞吐 (MB/s)", format!("{mb_s:.0}"));
    metrics.push("索引 (行/s)", format!("{lines_s:.0}"));
    metrics.push("索引单步 avg (ms)", format!("{avg_stall_ms:.2}"));
    metrics.push("索引单步 max (ms)", format!("{max_stall_ms:.2}"));
    (
        total_lines,
        IndexBenchResult { elapsed, max_stall },
        window_samples,
    )
}

/// Phase 2:在后台按 4096 行上限连续推进 Problems。前台同时竞争同一 Session
/// 锁并读取 200 行 All 窗口,因此延迟样本包含真实锁等待；单步计时覆盖
/// `scan_problems_step` 的 logcore Session 锁段,不包含 Tauri DTO/事件开销。
fn phase_problems(
    session: &mut Session,
    total_lines: usize,
    index_elapsed: Duration,
    mut window_samples: WindowLatencySamples,
    metrics: &mut Metrics,
) -> ProblemsBenchResult {
    println!("\n[Phase 2] Problems (每批 {PROBLEMS_CHUNK} 行,连续扫描墙钟中真实争锁读取窗口) ...");
    let shared = Mutex::new(session);
    let done = AtomicBool::new(false);
    let phase = AtomicUsize::new(BenchPhase::Initializing as usize);
    let start = Barrier::new(2);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            BenchPhase::CatchingUpProblems.store(&phase);
            start.wait();
            let started = Instant::now();
            let mut max_stall = Duration::ZERO;
            let mut scan_duration = Duration::ZERO;
            let mut steps = 0u64;
            loop {
                let mut guard = shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let step_started = Instant::now();
                let step = guard.scan_problems_step(PROBLEMS_CHUNK);
                drop(guard);
                let stall = step_started.elapsed();
                max_stall = max_stall.max(stall);
                scan_duration += stall;
                steps += 1;
                if step.caught_up {
                    break;
                }
                std::thread::yield_now();
            }
            BenchPhase::FinishPending.store(&phase);
            let mut guard = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let finish_started = Instant::now();
            let final_step = guard.finish_problem_input();
            drop(guard);
            let finish_stall = finish_started.elapsed();
            max_stall = max_stall.max(finish_stall);
            scan_duration += finish_stall;
            steps += 1;
            BenchPhase::Done.store(&phase);
            done.store(true, Ordering::Release);
            (
                started.elapsed(),
                max_stall,
                scan_duration,
                scan_duration.as_secs_f64() * 1000.0 / steps as f64,
                final_step.finished,
            )
        });

        start.wait();
        let problem_window_samples = sample_windows_until_done(
            &shared,
            &done,
            &phase,
            0xA0761D6478BD642F,
            WINDOW_SAMPLES / 2,
        );

        let (elapsed, max_stall, scan_duration, avg_stall_ms, finished) =
            worker.join().expect("Problems benchmark worker panicked");
        if !finished {
            fail("Problems 未在稳定输入末尾完成封口");
        }
        window_samples.extend(problem_window_samples);
        let window_summary = window_samples.active_summary();
        let p99 = window_summary.map(|summary| summary.p99);
        let concurrent_samples = window_samples.active_count();
        let standalone_lines_s = total_lines as f64 / elapsed.as_secs_f64();
        let cpu_lines_s = total_lines as f64 / scan_duration.as_secs_f64();
        let combined = index_elapsed + elapsed;
        let stats = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .problem_stats();
        let memory = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .problem_memory_stats();

        println!(
            "  完成墙钟: {:.2}s, {:.1} M行/s, index+Problems {:.2}s",
            elapsed.as_secs_f64(),
            standalone_lines_s / 1_000_000.0,
            combined.as_secs_f64()
        );
        println!(
            "  锁内 core-call CPU/热页诊断: {:.2}s, {:.1} M行/s",
            scan_duration.as_secs_f64(),
            cpu_lines_s / 1_000_000.0
        );
        println!(
            "  Problems core 锁段: 平均 {avg_stall_ms:.2}ms, 最大 {:.2}ms",
            max_stall.as_secs_f64() * 1000.0
        );
        println!(
            "  扫描期窗口: {} ({concurrent_samples} 个真实争锁样本)",
            format_latency_summary(window_summary)
        );
        print_window_phase_coverage(&window_samples);
        println!(
            "  事件: 检出 {} / 保存 {} / 分组 {} / limited={}",
            stats.observed_occurrence_count,
            stats.stored_occurrence_count,
            stats.stored_group_count,
            stats.limited
        );
        println!(
            "  受控内存: charged {:.2}MiB / retained {:.2}MiB / high-water {:.2}MiB / \
             budget {:.2}MiB / denied {}",
            memory.charged_bytes as f64 / MIB,
            memory.retained_heap_bytes as f64 / MIB,
            memory.high_water_charged_bytes as f64 / MIB,
            memory.limit_bytes as f64 / MIB,
            memory.denied_reservation_count
        );

        metrics.push(
            "Problems standalone 墙钟 (M行/s)",
            format!("{:.1}", standalone_lines_s / 1_000_000.0),
        );
        metrics.push(
            "Problems core-call CPU/热页 (M行/s)",
            format!("{:.1}", cpu_lines_s / 1_000_000.0),
        );
        metrics.push("Problems 耗时 (s)", format!("{:.2}", elapsed.as_secs_f64()));
        metrics.push(
            "index + Problems (s)",
            format!("{:.2}", combined.as_secs_f64()),
        );
        metrics.push(
            "Problems 单步 max (ms)",
            format!("{:.2}", max_stall.as_secs_f64() * 1000.0),
        );
        metrics.push(
            "Problems 扫描期窗口 (µs)",
            format!(
                "{}; phase index {} / catching-up {} / finish {}",
                format_latency_summary(window_summary),
                window_samples.indexing.len(),
                window_samples.catching_up.len(),
                window_samples.finish_pending.len()
            ),
        );
        metrics.push(
            "Problems 事件",
            format!(
                "observed {} / stored {} / groups {} / limited {}",
                stats.observed_occurrence_count,
                stats.stored_occurrence_count,
                stats.stored_group_count,
                stats.limited
            ),
        );
        metrics.push(
            "Problems 受控内存 (MiB)",
            format!(
                "charged {:.2} / retained {:.2} / high-water {:.2} / limit {:.2} / denied {}",
                memory.charged_bytes as f64 / MIB,
                memory.retained_heap_bytes as f64 / MIB,
                memory.high_water_charged_bytes as f64 / MIB,
                memory.limit_bytes as f64 / MIB,
                memory.denied_reservation_count
            ),
        );
        ProblemsBenchResult {
            standalone_lines_per_second: Some(standalone_lines_s),
            combined,
            max_stall,
            window_p99_micros: p99,
            concurrent_window_samples: concurrent_samples,
            indexing_window_samples: window_samples.indexing.len(),
            catching_up_window_samples: window_samples.catching_up.len(),
            observed_occurrence_count: stats.observed_occurrence_count,
            stored_occurrence_count: stats.stored_occurrence_count,
            stored_group_count: stats.stored_group_count,
            stats_limited: stats.limited,
            memory_high_water_bytes: memory.high_water_charged_bytes,
            memory_retained_bytes: memory.retained_heap_bytes,
        }
    })
}

fn assert_problem_contracts(
    options: &BenchOptions,
    actual_bytes: u64,
    total_lines: usize,
    index: IndexBenchResult,
    problems: ProblemsBenchResult,
) -> bool {
    let mut failures = Vec::new();
    if options.corpus == CorpusKind::Sparse {
        let min_bytes = ACCEPTANCE_CORPUS_BYTES.saturating_sub(ACCEPTANCE_CORPUS_BYTE_TOLERANCE);
        let max_bytes = ACCEPTANCE_CORPUS_BYTES.saturating_add(ACCEPTANCE_CORPUS_BYTE_TOLERANCE);
        if !(min_bytes..=max_bytes).contains(&actual_bytes) {
            failures.push(format!(
                "sparse 硬验收要求约 10GiB,实际 {:.3}GiB 不在 ±1% 范围",
                actual_bytes as f64 / GIB
            ));
        }
        if !(ACCEPTANCE_MIN_LINES..=ACCEPTANCE_MAX_LINES).contains(&total_lines) {
            failures.push(format!(
                "sparse 硬验收要求约 7115 万行,实际 {total_lines} 不在 \
                 {ACCEPTANCE_MIN_LINES}..={ACCEPTANCE_MAX_LINES}"
            ));
        }
    }
    if index.max_stall.as_secs_f64() * 1_000.0 > MAX_INDEX_STALL_MILLIS {
        failures.push(format!(
            "索引单步 {:.2}ms > {MAX_INDEX_STALL_MILLIS:.0}ms",
            index.max_stall.as_secs_f64() * 1_000.0
        ));
    }
    if problems.max_stall.as_secs_f64() * 1_000.0 > MAX_PROBLEMS_STALL_MILLIS {
        failures.push(format!(
            "Problems 单步 {:.2}ms > {MAX_PROBLEMS_STALL_MILLIS:.0}ms",
            problems.max_stall.as_secs_f64() * 1_000.0
        ));
    }
    if problems.indexing_window_samples == 0 {
        failures.push("窗口采样没有覆盖 Indexing phase".to_string());
    }
    if problems.catching_up_window_samples == 0 {
        failures.push("窗口采样没有覆盖 CatchingUpProblems phase".to_string());
    }
    if options.corpus == CorpusKind::Sparse
        && problems.concurrent_window_samples < MIN_CONCURRENT_WINDOW_SAMPLES
    {
        failures.push(format!(
            "仅 {} 个真实争锁窗口样本,少于有效门槛 {}",
            problems.concurrent_window_samples, MIN_CONCURRENT_WINDOW_SAMPLES
        ));
    }
    if problems.memory_high_water_bytes > DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES {
        failures.push(format!(
            "Problems high-water {} bytes > 逻辑预算 {} bytes",
            problems.memory_high_water_bytes, DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES
        ));
    }
    if problems.memory_retained_bytes > MAX_PROBLEMS_RETAINED_BYTES {
        failures.push(format!(
            "Problems retained {} bytes > 目标 {} bytes",
            problems.memory_retained_bytes, MAX_PROBLEMS_RETAINED_BYTES
        ));
    }

    match options.corpus {
        CorpusKind::Sparse => {
            match problems.window_p99_micros {
                Some(p99) if p99 > MAX_PROBLEMS_WINDOW_P99_MICROS => {
                    failures.push(format!(
                        "扫描期 get_rows(200) p99 {p99}µs > \
                         {MAX_PROBLEMS_WINDOW_P99_MICROS}µs"
                    ));
                }
                None => failures.push("扫描期 get_rows(200) 没有有效样本".to_string()),
                Some(_) => {}
            }
            if options.schedule == BenchSchedule::Sequential {
                match problems.standalone_lines_per_second {
                    Some(lines_per_second) if lines_per_second < MIN_PROBLEMS_LINES_PER_SECOND => {
                        failures.push(format!(
                            "Problems 连续扫描墙钟吞吐 {:.2}M行/s < {:.2}M行/s",
                            lines_per_second / 1_000_000.0,
                            MIN_PROBLEMS_LINES_PER_SECOND / 1_000_000.0
                        ));
                    }
                    None => failures
                        .push("sequential 调度没有产生 Problems standalone 墙钟吞吐".to_string()),
                    Some(_) => {}
                }
            }
            if options.schedule == BenchSchedule::Production
                && problems.combined.as_secs_f64() > MAX_INDEX_AND_PROBLEMS_SECONDS
            {
                failures.push(format!(
                    "index + Problems {:.2}s > {MAX_INDEX_AND_PROBLEMS_SECONDS:.0}s",
                    problems.combined.as_secs_f64()
                ));
            }
            if problems.stats_limited {
                failures.push("稀疏语料不应触发 limited".to_string());
            }
            let expected = expected_sparse_occurrences(total_lines, actual_bytes);
            if problems.observed_occurrence_count != expected {
                failures.push(format!(
                    "sparse observed {} != corpus oracle {expected}",
                    problems.observed_occurrence_count
                ));
            }
            if problems.stored_occurrence_count != expected {
                failures.push(format!(
                    "sparse stored {} != corpus oracle {expected}",
                    problems.stored_occurrence_count
                ));
            }
            let expected_groups = u32::from(expected != 0);
            if problems.stored_group_count != expected_groups {
                failures.push(format!(
                    "sparse groups {} != 高重复 fingerprint oracle {expected_groups}",
                    problems.stored_group_count
                ));
            }
        }
        CorpusKind::Storm => {
            if !problems.stats_limited {
                failures.push("事件风暴语料必须触发 limited".to_string());
            }
            let expected = expected_storm_occurrences(total_lines);
            if problems.observed_occurrence_count != expected {
                failures.push(format!(
                    "storm observed {} != corpus oracle {expected}",
                    problems.observed_occurrence_count
                ));
            }
            if problems.stored_occurrence_count > MAX_STORED_PROBLEM_OCCURRENCES {
                failures.push(format!(
                    "storm stored occurrences {} > 结构上限 {}",
                    problems.stored_occurrence_count, MAX_STORED_PROBLEM_OCCURRENCES
                ));
            }
            if problems.stored_group_count > MAX_STORED_PROBLEM_GROUPS {
                failures.push(format!(
                    "storm groups {} > 结构上限 {}",
                    problems.stored_group_count, MAX_STORED_PROBLEM_GROUPS
                ));
            }
            let expected_stored = expected.min(u64::from(MAX_STORED_PROBLEM_GROUPS));
            if problems.stored_occurrence_count != expected_stored {
                failures.push(format!(
                    "storm stored {} != distinct-fingerprint/cap oracle {expected_stored}",
                    problems.stored_occurrence_count
                ));
            }
            if u64::from(problems.stored_group_count) != expected_stored {
                failures.push(format!(
                    "storm groups {} != distinct-fingerprint/cap oracle {expected_stored}",
                    problems.stored_group_count
                ));
            }
        }
    }

    if !failures.is_empty() {
        fail(&format!(
            "Problems 硬门槛失败:\n  - {}",
            failures.join("\n  - ")
        ));
    }
    options.corpus != CorpusKind::Sparse || options.schedule == BenchSchedule::Sequential
}

fn expected_sparse_occurrences(total_lines: usize, actual_bytes: u64) -> u64 {
    let generated_lines = total_lines.saturating_sub(CORPUS_HEADER_LINES);
    let periodic = if generated_lines < 13 {
        0
    } else {
        ((generated_lines - 13) / 1_000_000 + 1) as u64
    };
    periodic + u64::from(actual_bytes >= CORPUS_BOUNDARY_BYTES)
}

fn expected_storm_occurrences(total_lines: usize) -> u64 {
    total_lines.saturating_sub(CORPUS_HEADER_LINES) as u64
}

/// 计时跑一遍分块过滤,报告耗时/百万行每秒/命中数,返回命中数组。
fn run_filter(
    session: &Session,
    spec: &FilterSpec,
    total_lines: usize,
    label: &str,
    metrics: &mut Metrics,
) -> Vec<u32> {
    let matcher =
        FilterMatcher::new(spec).unwrap_or_else(|e| fail(&format!("过滤器编译失败: {e:?}")));
    let t0 = Instant::now();
    let mut matches = Vec::new();
    let mut start = 0;
    while start < total_lines {
        let end = (start + FILTER_CHUNK).min(total_lines);
        matches.extend(session.filter_indexed_range(&matcher, start, end));
        start = end;
    }
    let elapsed = t0.elapsed();
    let mlines_s = (total_lines as f64 / 1_000_000.0) / elapsed.as_secs_f64();
    println!(
        "  {label}: {} 命中, {:.2}s, {mlines_s:.1} M行/s",
        matches.len(),
        elapsed.as_secs_f64()
    );
    metrics.push(
        format!("过滤 {label} (M行/s)"),
        format!("{mlines_s:.1} (命中 {})", matches.len()),
    );
    matches
}

/// Phase 3:四种过滤器分别计时。返回 (b) 的命中数组供后续复用。
fn phase_filters(session: &Session, total_lines: usize, metrics: &mut Metrics) -> Vec<u32> {
    println!("\n[Phase 3] 过滤 (4096 行分块) ...");

    // a) 级别位掩码 E|F。
    let spec_a = FilterSpec {
        levels: LevelMask::from_levels(&["E", "F"]),
        ..Default::default()
    };
    run_filter(session, &spec_a, total_lines, "a 级别 E|F", metrics);

    // b) tag 包含 明文 "NetPolicy|WifiService"。
    let spec_b = FilterSpec {
        tag_include: FilterField::plain(true, "NetPolicy|WifiService"),
        ..Default::default()
    };
    let matches_b = run_filter(
        session,
        &spec_b,
        total_lines,
        "b tag NetPolicy|WifiService",
        metrics,
    );

    // c) 关键词 明文单词,中等命中率。"connection" 出现在部分消息里。
    let spec_c = FilterSpec {
        word_include: FilterField::plain(true, "connection"),
        ..Default::default()
    };
    run_filter(session, &spec_c, total_lines, "c word connection", metrics);

    // d) 关键词 正则 conn.*(timeout|reset)。
    // 过滤字段以 `|` 切分多值(见 filter.rs split_values),因此正则里不能出现顶层
    // `|`;改写成两个正则值 `conn.*timeout|conn.*reset`,contains_any 命中任一即匹配,
    // 语义与 conn.*(timeout|reset) 等价。
    let spec_d = FilterSpec {
        word_include: FilterField::regex(true, "conn.*timeout|conn.*reset"),
        ..Default::default()
    };
    run_filter(
        session,
        &spec_d,
        total_lines,
        "d regex conn.*timeout|conn.*reset",
        metrics,
    );

    matches_b
}

/// 计算 min/median/p99/max(微秒)。
fn latency_stats(mut samples: Vec<u128>) -> (u128, u128, u128, u128) {
    samples.sort_unstable();
    let n = samples.len();
    let median = samples[n / 2];
    let p99_index = n
        .saturating_mul(99)
        .div_ceil(100)
        .saturating_sub(1)
        .min(n - 1);
    let p99 = samples[p99_index];
    (samples[0], median, p99, samples[n - 1])
}

/// Phase 4:200 行窗口读取,100 个伪随机偏移,分别测 All 与 Filtered。
fn phase_window_reads(
    session: &mut Session,
    total_lines: usize,
    filter_2b: &[u32],
    metrics: &mut Metrics,
) {
    println!("\n[Phase 4] 窗口读取 ({WINDOW_ROWS} 行 × {WINDOW_SAMPLES} 次随机偏移) ...");
    let mut rng = Rng::new(0xD1B54A32D192ED03);

    // All 视图。
    let all_span = total_lines.saturating_sub(WINDOW_ROWS).max(1);
    let mut all_samples = Vec::with_capacity(WINDOW_SAMPLES);
    for _ in 0..WINDOW_SAMPLES {
        let start = rng.below(all_span);
        let t0 = Instant::now();
        let rows = session.get_rows_for_view(RowsView::All, start, WINDOW_ROWS);
        all_samples.push(t0.elapsed().as_micros());
        std::hint::black_box(rows.len());
    }
    let (min, med, p99, max) = latency_stats(all_samples);
    println!("  All      : min {min}µs  median {med}µs  p99 {p99}µs  max {max}µs");
    metrics.push(
        "窗口 All (µs)",
        format!("min {min} / med {med} / p99 {p99} / max {max}"),
    );

    // 应用 Phase-2b 过滤后测 Filtered 视图。
    let spec_b = FilterSpec {
        tag_include: FilterField::plain(true, "NetPolicy|WifiService"),
        ..Default::default()
    };
    let filtered_len = session.apply_filter_results(&spec_b, filter_2b.to_vec());
    let filt_span = filtered_len.saturating_sub(WINDOW_ROWS).max(1);
    let mut filt_samples = Vec::with_capacity(WINDOW_SAMPLES);
    for _ in 0..WINDOW_SAMPLES {
        let start = rng.below(filt_span);
        let t0 = Instant::now();
        let rows = session.get_rows_for_view(RowsView::Filtered, start, WINDOW_ROWS);
        filt_samples.push(t0.elapsed().as_micros());
        std::hint::black_box(rows.len());
    }
    let (min, med, p99, max) = latency_stats(filt_samples);
    println!(
        "  Filtered : min {min}µs  median {med}µs  p99 {p99}µs  max {max}µs (视图 {filtered_len} 行)"
    );
    metrics.push(
        "窗口 Filtered (µs)",
        format!("min {min} / med {med} / p99 {p99} / max {max}"),
    );
}

/// Phase 5:明文大小写不敏感搜索,分块 search_indexed_range。
fn phase_search(session: &Session, total_lines: usize, metrics: &mut Metrics) {
    println!("\n[Phase 5] 搜索 (明文大小写不敏感, 分块) ...");
    let spec = SearchSpec {
        query: "TIMEOUT".to_string(),
        regex: false,
        case_sensitive: false,
    };
    let matcher =
        SearchMatcher::new(&spec).unwrap_or_else(|e| fail(&format!("搜索器编译失败: {e:?}")));
    let t0 = Instant::now();
    let mut matches = Vec::new();
    let mut start = 0;
    while start < total_lines {
        let end = (start + FILTER_CHUNK).min(total_lines);
        matches.extend(session.search_indexed_range(&matcher, start, end));
        start = end;
    }
    let elapsed = t0.elapsed();
    let mlines_s = (total_lines as f64 / 1_000_000.0) / elapsed.as_secs_f64();
    println!(
        "  \"timeout\" (不敏感): {} 命中, {:.2}s, {mlines_s:.1} M行/s",
        matches.len(),
        elapsed.as_secs_f64()
    );
    metrics.push(
        "搜索 (M行/s)",
        format!("{mlines_s:.1} (命中 {})", matches.len()),
    );
}

/// Phase 6:导出 Filtered(3b 过滤)与 All,报告 MB/s,结束后删除输出文件。
fn phase_export(session: &mut Session, source: &Path, metrics: &mut Metrics) {
    println!("\n[Phase 6] 导出 ...");
    let dir = source.parent().unwrap_or_else(|| Path::new("/tmp"));

    // Filtered 导出:沿用当前已应用的 2b 过滤。
    let filtered_out = dir.join("logfilter-bench-export-filtered.log");
    let t0 = Instant::now();
    let summary = session
        .export_view(RowsView::Filtered, &filtered_out)
        .unwrap_or_else(|e| fail(&format!("Filtered 导出失败: {e}")));
    let elapsed = t0.elapsed();
    let mb_s = (summary.written_bytes as f64 / MIB) / elapsed.as_secs_f64();
    println!(
        "  Filtered: {} 行 {:.2} MiB, {:.2}s, {mb_s:.0} MB/s",
        summary.written_lines,
        summary.written_bytes as f64 / MIB,
        elapsed.as_secs_f64()
    );
    metrics.push(
        "导出 Filtered (MB/s)",
        format!("{mb_s:.0} ({} 行)", summary.written_lines),
    );
    let _ = fs::remove_file(&filtered_out);

    // All 导出。
    let all_out = dir.join("logfilter-bench-export-all.log");
    let t0 = Instant::now();
    let summary = session
        .export_view(RowsView::All, &all_out)
        .unwrap_or_else(|e| fail(&format!("All 导出失败: {e}")));
    let elapsed = t0.elapsed();
    let mb_s = (summary.written_bytes as f64 / MIB) / elapsed.as_secs_f64();
    println!(
        "  All     : {} 行 {:.2} GiB, {:.2}s, {mb_s:.0} MB/s",
        summary.written_lines,
        summary.written_bytes as f64 / GIB,
        elapsed.as_secs_f64()
    );
    metrics.push(
        "导出 All (MB/s)",
        format!("{mb_s:.0} ({} 行)", summary.written_lines),
    );
    let _ = fs::remove_file(&all_out);
}

fn fail(msg: &str) -> ! {
    eprintln!("bench 错误: {msg}");
    std::process::exit(1);
}
