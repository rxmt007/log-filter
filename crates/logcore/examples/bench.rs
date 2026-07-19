//! 大文件基准:生成合成的 threadtime logcat,测量引擎核心热路径,
//! 用真实数字验证"10GB 文件不崩不卡",并作为后续改动的回归基线。
//!
//! 用法: cargo run --release -p logcore --example bench -- [size_gb] [file_path]
//!   size_gb   目标大小(GiB,f64),默认 2.0
//!   file_path 合成日志路径,默认 /tmp/logfilter-bench.log
//!             若文件已存在且大小在目标 ±5% 内则复用(跳过生成)。
//!
//! macOS 无 /proc,进程内不测 RSS;峰值内存请用
//!   /usr/bin/time -l cargo run --release -p logcore --example bench -- ...
//! 取输出里的 "maximum resident set size"。

use logcore::filter::{FilterField, FilterMatcher, FilterSpec, LevelMask};
use logcore::search::{SearchMatcher, SearchSpec};
use logcore::session::{RowsView, Session};
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// 架构铁律:UI 窗口只取可见窗口。基准沿用应用中的口径。
const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 后台索引步进预算,与应用一致
const FILTER_CHUNK: usize = 4096; // 分块扫描步长,与应用一致
const WINDOW_ROWS: usize = 200; // UI 交互窗口一次取的行数
const WINDOW_SAMPLES: usize = 100; // 随机窗口读取采样次数

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
    let mut args = std::env::args().skip(1);
    let size_gb: f64 = args
        .next()
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| fail("size_gb 必须是数字,例如 2.0"))
        })
        .unwrap_or(2.0);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/logfilter-bench.log"));

    if size_gb <= 0.0 {
        fail("size_gb 必须为正数");
    }

    let target_bytes = (size_gb * GIB) as u64;
    println!(
        "目标大小 {size_gb:.2} GiB (~{target_bytes} bytes)  路径 {}",
        path.display()
    );

    let mut metrics = Metrics::new();
    metrics.push("目标大小 (GiB)", format!("{size_gb:.2}"));

    // Phase G:生成(可复用则跳过)。
    let actual_bytes = ensure_corpus(&path, target_bytes, &mut metrics);
    metrics.push(
        "文件字节数",
        format!("{actual_bytes} ({:.2} GiB)", actual_bytes as f64 / GIB),
    );

    // Phase 1:索引。
    let mut session = Session::open(&path).unwrap_or_else(|e| fail(&format!("打开失败: {e}")));
    let total_lines = phase_index(&mut session, actual_bytes, &mut metrics);

    // Phase 2:过滤。
    let filter_2b = phase_filters(&session, total_lines, &mut metrics);

    // Phase 3:窗口读取(All + Filtered)。
    phase_window_reads(&mut session, total_lines, &filter_2b, &mut metrics);

    // Phase 4:搜索。
    phase_search(&session, total_lines, &mut metrics);

    // Phase 5:导出(Filtered 用 2b 过滤,All 全量)。
    phase_export(&mut session, &path, &mut metrics);

    metrics.print();
    println!(
        "\n提示: macOS 无 /proc,进程内不测 RSS。峰值内存请用\n  \
         /usr/bin/time -l cargo run --release -p logcore --example bench -- {size_gb} {}\n  \
         读取输出中的 \"maximum resident set size\"。",
        path.display()
    );
}

/// 保证语料存在:大小在目标 ±5% 内则复用,否则重新生成。返回实际字节数。
fn ensure_corpus(path: &Path, target_bytes: u64, metrics: &mut Metrics) -> u64 {
    if let Ok(meta) = fs::metadata(path) {
        let existing = meta.len();
        let low = target_bytes as f64 * 0.95;
        let high = target_bytes as f64 * 1.05;
        if (existing as f64) >= low && (existing as f64) <= high {
            println!(
                "复用已存在文件: {existing} bytes ({:.2} GiB) 在目标 ±5% 内,跳过生成。",
                existing as f64 / GIB
            );
            metrics.push("生成", "复用(跳过)".to_string());
            return existing;
        }
        println!("已存在文件 {existing} bytes 超出目标 ±5%,重新生成。",);
    }
    generate(path, target_bytes, metrics)
}

/// Phase G:写 threadtime 行,BufWriter 8MB 缓冲。返回写出的字节数。
fn generate(path: &Path, target_bytes: u64, metrics: &mut Metrics) -> u64 {
    println!("生成中 ... (每 512MB 报告一次进度)");
    let file = fs::File::create(path).unwrap_or_else(|e| fail(&format!("创建文件失败: {e}")));
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
    let mut rng = Rng::new(0x9E3779B97F4A7C15);

    // pid/tid 池:约 30 个 pid。
    let pids: Vec<u32> = (0..30).map(|i| 1000 + i * 137).collect();

    let mut written: u64 = 0;
    let mut line_no: u64 = 0;
    let mut next_progress: u64 = 512 * 1024 * 1024;

    // 时间戳单调推进,从固定起点起,每行涨 1~9 毫秒。
    let mut secs: u64 = 0; // 自 00:00:00.000 起的毫秒计数
    let month = 7u32;
    let day = 6u32;
    let mut msg = String::with_capacity(1200);
    let t0 = Instant::now();

    let mut result: io::Result<()> = Ok(());
    while written < target_bytes {
        line_no += 1;
        secs += 1 + (rng.next_u64() % 9);
        let ms = secs % 1000;
        let total_s = secs / 1000;
        let hh = (total_s / 3600) % 24;
        let mm = (total_s / 60) % 60;
        let ss = total_s % 60;

        // 每约 100 万行插入一条原始分隔行(不符合 threadtime,考验解析健壮性)。
        if line_no.is_multiple_of(1_000_000) {
            if let Err(e) = writer.write_all(b"--------- beginning of main\n") {
                result = Err(e);
                break;
            }
            written += 28;
            continue;
        }

        let pid = pids[rng.below(pids.len())];
        let tid = pid + (rng.next_u64() % 8) as u32; // tid 贴近 pid
        let level = pick_level(&mut rng);
        let tag = TAGS[rng.below(TAGS.len())];

        build_message(&mut rng, line_no, &mut msg);

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

/// Phase 1:打开后按 8MB 预算步进索引至完成。返回总行数。
fn phase_index(session: &mut Session, total_bytes: u64, metrics: &mut Metrics) -> usize {
    println!("\n[Phase 1] 索引 (每 1GB 报告进度) ...");
    let t0 = Instant::now();
    let mut steps: u64 = 0;
    let mut max_stall = Duration::ZERO;
    let mut sum_stall = Duration::ZERO;
    let mut next_progress: usize = 1024 * 1024 * 1024;

    loop {
        let step_start = Instant::now();
        let done = session.index_step(INDEX_BUDGET);
        let stall = step_start.elapsed();
        steps += 1;
        sum_stall += stall;
        if stall > max_stall {
            max_stall = stall;
        }
        let indexed = session.indexed_bytes();
        if indexed >= next_progress {
            println!(
                "  已索引 {:.2} GiB / {:.2} GiB ({} 行)",
                indexed as f64 / GIB,
                total_bytes as f64 / GIB,
                session.total_lines()
            );
            next_progress += 1024 * 1024 * 1024;
        }
        if done {
            break;
        }
    }

    let elapsed = t0.elapsed();
    let total_lines = session.total_lines();
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
    total_lines
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

/// Phase 2:四种过滤器分别计时。返回 (b) 的命中数组供后续复用。
fn phase_filters(session: &Session, total_lines: usize, metrics: &mut Metrics) -> Vec<u32> {
    println!("\n[Phase 2] 过滤 (4096 行分块) ...");

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
    let p99 = samples[((n * 99) / 100).min(n - 1)];
    (samples[0], median, p99, samples[n - 1])
}

/// Phase 3:200 行窗口读取,100 个伪随机偏移,分别测 All 与 Filtered。
fn phase_window_reads(
    session: &mut Session,
    total_lines: usize,
    filter_2b: &[u32],
    metrics: &mut Metrics,
) {
    println!("\n[Phase 3] 窗口读取 ({WINDOW_ROWS} 行 × {WINDOW_SAMPLES} 次随机偏移) ...");
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

/// Phase 4:明文大小写不敏感搜索,分块 search_indexed_range。
fn phase_search(session: &Session, total_lines: usize, metrics: &mut Metrics) {
    println!("\n[Phase 4] 搜索 (明文大小写不敏感, 分块) ...");
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

/// Phase 5:导出 Filtered(2b 过滤)与 All,报告 MB/s,结束后删除输出文件。
fn phase_export(session: &mut Session, source: &Path, metrics: &mut Metrics) {
    println!("\n[Phase 5] 导出 ...");
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
