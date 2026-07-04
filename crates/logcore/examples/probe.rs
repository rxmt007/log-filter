//! 无头验证探针:打开一个(大)日志文件,增量建满索引,随机读几行。
//! 用法: cargo run --release --example probe -- <文件路径>
use logcore::session::Session;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");

    let t0 = Instant::now();
    let mut s = Session::open(&PathBuf::from(&path)).expect("open failed");
    let total_bytes = s.total_bytes();
    // 像应用的后台线程那样,按 8MB 预算步进,直到建满索引。
    while !s.index_step(8 * 1024 * 1024) {}
    let index_ms = t0.elapsed().as_millis();
    let lines = s.total_lines();
    println!("bytes={total_bytes} lines={lines} index_ms={index_ms}");

    // 随机访问:读头部和尾部各 3 行,证明靠偏移索引可 O(1) 跳转。
    let t1 = Instant::now();
    let head = s.get_rows(0, 3);
    let tail = s.get_rows(lines.saturating_sub(3), 3);
    let read_us = t1.elapsed().as_micros();
    println!("read 6 rows (head+tail) in {read_us}us");
    for (no, e) in head.iter().chain(tail.iter()) {
        println!("  #{no} {} {}({}/{}) {}", e.time, e.tag, e.pid, e.tid, e.message);
    }
}
