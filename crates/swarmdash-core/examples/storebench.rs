//! Fills a log store with realistic lines and times what the UI asks of it.
//!
//!     cargo run --release -p swarmdash-core --example storebench -- <store.db> [million lines] [services]
//!
//! An existing store is reused, so the fill only runs once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use swarmdash_core::docker::frames::StreamKind;
use swarmdash_core::logs::Level;
use swarmdash_core::logs::assemble::Assembler;
use swarmdash_core::model::LogSource;
use swarmdash_core::store::{ExportFormat, LogFilter, Store};

const DAY: i64 = 86_400_000_000;

fn source(i: usize) -> LogSource {
    LogSource {
        key: format!("container{i:04}"),
        via_task: false,
        stack: format!("stack{}", i % 8),
        service: format!("stack{}_svc{i}", i % 8),
        replica: "1".into(),
        node: "node1".into(),
        tty: false,
        running: true,
    }
}

/// One line of a service: mostly requests, some warnings, errors and traces.
fn lines(n: u64, out: &mut Vec<String>) {
    match n % 50 {
        0 => {
            out.push(format!("2026-10-27 10:00:00.000 ERROR 1 --- [exec-{}] c.a.Checkout : Checkout failed for cart {n}", n % 8));
            out.push(format!("java.lang.IllegalStateException: cart {n} is empty"));
            for f in 0..12 {
                out.push(format!("\tat com.acme.shop.Layer{f}.call(Layer{f}.java:{})", 40 + f));
            }
        }
        7 | 19 => out.push(format!("2026-10-27 10:00:00.000  WARN 1 --- [exec-1] c.a.Db : Slow query took {}ms", n % 4000)),
        13 => out.push(format!("2026-10-27 10:00:00.000 ERROR 1 --- [exec-2] c.a.Pay : Upstream {} timed out after {}ms", n % 5, n % 900)),
        _ => out.push(format!(
            "2026-10-27 10:00:00.000  INFO 1 --- [exec-{}] c.a.Web : GET /api/items/{n}?page={} 200 in {}ms user=u{}",
            n % 8,
            n % 30,
            n % 90,
            n % 1000
        )),
    }
}

fn fill(store: &Store, total: u64, services: usize, now: i64) {
    let started = Instant::now();
    let span = 7 * DAY;
    let srcs: Vec<LogSource> = (0..services).map(source).collect();
    let mut asm: Vec<Assembler> = srcs.iter().map(|s| Assembler::new(&s.service)).collect();
    let mut buf = Vec::new();
    let mut batches: Vec<Vec<swarmdash_core::logs::LogEntry>> = vec![Vec::new(); services];
    let mut written = 0u64;
    let mut n = 0u64;
    while written < total {
        n += 1;
        // A third of everything comes from one chatty service.
        let i = if n.is_multiple_of(3) { 0 } else { (n as usize * 7919) % services };
        let ts = now - span + (n as i64 * span / (total as i64 * 11 / 10)).min(span - 1);
        buf.clear();
        lines(n, &mut buf);
        for (k, l) in buf.iter().enumerate() {
            asm[i].push(ts + k as i64, StreamKind::Stdout, l, &mut batches[i]);
        }
        if batches[i].len() >= 2000 {
            written += batches[i].len() as u64;
            store.insert("bench", &srcs[i], &batches[i]).unwrap();
            batches[i].clear();
        }
    }
    for (i, b) in batches.iter_mut().enumerate() {
        asm[i].flush(b);
        store.insert("bench", &srcs[i], b).unwrap();
    }
    let s = started.elapsed().as_secs_f64();
    println!("fill: {written} entries in {s:.1} s ({:.0}/s)", written as f64 / s);
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{label:<44} {:>8.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    r
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("bench.db"));
    let millions: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let services: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let fresh = !path.exists();
    let store = Arc::new(Store::open(&path).unwrap());
    let now = chrono::Utc::now().timestamp_micros();
    time("upgrade (service index)", || store.upgrade().unwrap());
    if fresh {
        fill(&store, (millions * 1e6) as u64, services, now);
    }
    // The fill ends "now" at the time it ran; queries use the newest line as now.
    let all = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
    let newest = store.query("bench", &all, 1, 1).unwrap().entries.first().map(|e| e.ts).unwrap_or(now) + 1;
    let stats = time("stats", || store.stats().unwrap());
    println!("  {} entries, {} MB", stats.entries, stats.bytes / 1_048_576);

    let window = |mins: i64| LogFilter { from: newest - mins * 60_000_000, to: newest, ..Default::default() };
    for (label, mins) in [("15m", 15), ("1h", 60), ("24h", 1440), ("7d", 10080)] {
        time(&format!("query {label}"), || store.query("bench", &window(mins), 60, 300).unwrap());
    }
    let mut f = window(10080);
    f.apply_query("timed");
    time("query 7d text 'timed'", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(10080);
    f.apply_query("cart 4242");
    time("query 7d text 'cart 4242'", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(10080);
    f.exact_services = vec!["stack3_svc11".into()];
    time("query 7d one quiet service", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(10080);
    f.apply_query("service:svc1");
    time("query 7d service:svc1 (substring)", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(10080);
    f.pick_stacks = vec!["stack3".into()];
    time("query 7d one stack", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(1440);
    f.exact_services = vec!["stack0_svc0".into()];
    time("query 24h chatty service", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(1440);
    f.apply_query("node:node1");
    time("query 24h node:node1", || store.query("bench", &f, 60, 300).unwrap());
    let mut f = window(10080);
    f.levels = vec![Level::Error];
    time("query 7d errors only", || store.query("bench", &f, 60, 5000).unwrap());
    time("query 7d limit 5000", || store.query("bench", &window(10080), 60, 5000).unwrap());
    time("error_summary", || store.error_summary("bench", newest).unwrap());
    time("error_rate 15m", || store.error_rate("bench", newest - 15 * 60_000_000, newest).unwrap());
    time("usage", || store.usage("bench").unwrap());
    let mut sink = std::io::sink();
    time("export 24h (log)", || store.export("bench", &window(1440), ExportFormat::Log, &mut sink).unwrap());

    // Lock contention: a writer inserts like the collector while the UI reads.
    let stop = Arc::new(AtomicBool::new(false));
    let worst_insert = Arc::new(AtomicU64::new(0));
    let writer = {
        let (store, stop, worst) = (store.clone(), stop.clone(), worst_insert.clone());
        std::thread::spawn(move || {
            let src = source(0);
            let mut a = Assembler::new(&src.service);
            let mut n = 10_000_000u64;
            let mut buf = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let mut batch = Vec::new();
                for _ in 0..200 {
                    n += 1;
                    buf.clear();
                    lines(n, &mut buf);
                    for l in &buf {
                        a.push(newest + n as i64, StreamKind::Stdout, l, &mut batch);
                    }
                }
                let t = Instant::now();
                store.insert("bench", &src, &batch).unwrap();
                worst.fetch_max(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    time("query 7d while writing", || store.query("bench", &window(10080), 60, 300).unwrap());
    time("error_summary while writing", || store.error_summary("bench", newest).unwrap());
    let mut sink = std::io::sink();
    time("export 7d while writing", || store.export("bench", &window(10080), ExportFormat::Jsonl, &mut sink).unwrap());
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    println!("{:<44} {:>8.1} ms", "worst insert of 200 lines while reading", worst_insert.load(Ordering::Relaxed) as f64 / 1000.0);
}
