//! Times log polls against the local Docker engine.
//!
//!     cargo run --release -p stackline-core --example collectbench -- [polls]

use std::sync::Arc;
use std::time::Instant;

use stackline_core::collector::Collector;
use stackline_core::docker::{DockerClient, Endpoint};
use stackline_core::snapshot::{self, Options};
use stackline_core::store::Store;

#[tokio::main]
async fn main() {
    let polls: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let client = DockerClient::new(Endpoint::local_default());
    let cpu = Default::default();
    let snap = snapshot::fetch(&client, &Options { stats: false, cpu: &cpu, oom: &Default::default() }).await.unwrap();
    let sources = snap.log_sources();
    println!("{} sources", sources.len());
    let dir = std::env::temp_dir().join(format!("collectbench-{}", std::process::id()));
    let store = Arc::new(Store::open(&dir.join("logs.db")).unwrap());
    let mut c = Collector::new("local", store.clone());
    for i in 0..polls {
        let t = Instant::now();
        let r = c.poll(&client, sources.clone(), 7 * 86_400_000_000).await;
        println!("poll {i}: {} entries in {:.0} ms {:?}", r.entries, t.elapsed().as_secs_f64() * 1000.0, r.errors);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}
