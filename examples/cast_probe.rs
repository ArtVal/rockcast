//! Live Cast discovery probe (mDNS + subnet scan).
//! Run: `cargo run --example cast_probe`

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let timeout = std::time::Duration::from_secs(8);
    println!("scanning for Cast devices (timeout {timeout:?})…");
    match rockcast::cast::discovery::discover(timeout) {
        Ok(list) => {
            println!("found {} device(s):", list.len());
            for d in list {
                println!(
                    "  - {} [{}] {}:{} id={}",
                    d.name, d.model, d.host, d.port, d.id
                );
            }
        }
        Err(e) => {
            eprintln!("discovery failed: {e}");
            std::process::exit(1);
        }
    }
}
