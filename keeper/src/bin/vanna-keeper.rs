//! `vanna-keeper <command> --config keeper.toml`
//!
//! - `run`: tick forever (`tick_secs` apart).
//! - `once`: one tick, then exit (for cron, or a first look with `dry_run`).
//! - `plan`: read the vault and print what the keeper would do now; sends nothing.

use vanna_keeper::keeper::log;
use vanna_keeper::live::LiveBackend;
use vanna_keeper::{Keeper, KeeperConfig};

fn usage() -> ! {
    eprintln!("usage: vanna-keeper <run|once|plan> --config <keeper.toml>");
    std::process::exit(2);
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or_else(|| usage());
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("keeper.toml");
    let config = KeeperConfig::load(config_path)?;
    let mut settings = config.settings()?;
    if command == "plan" {
        settings.dry_run = true;
    }
    let backend = LiveBackend::new(&config)?;
    let mut keeper = Keeper::new(backend, settings);
    log(&format!(
        "vanna-keeper: vault {} on {}{}",
        config.vault_id,
        config.rpc_url,
        if keeper.settings.dry_run { " (dry run)" } else { "" }
    ));
    match command {
        "run" => keeper.run(config.tick_secs),
        "once" | "plan" => {
            let report = keeper.tick()?;
            for (d, sig) in &report.executed {
                println!("{} {sig}: {}", d.action.name(), d.reason);
            }
            for (d, why) in &report.failed {
                println!("FAILED {}: {} ({why})", d.action.name(), d.reason);
            }
            for d in &report.planned {
                println!("would {}: {}", d.action.name(), d.reason);
            }
            if report.planned.is_empty() && report.executed.is_empty() && report.failed.is_empty() {
                println!("nothing to do");
            }
            for a in &report.alerts {
                println!("ALERT {a}");
            }
            Ok(())
        }
        _ => usage(),
    }
}
