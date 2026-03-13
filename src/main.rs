mod curl;
mod stats;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;
use clap::Parser;
use crossterm::style::{Color, Print, ResetColor, SetAttribute, SetForegroundColor, Attribute};
use crossterm::ExecutableCommand;
use envconfig::Envconfig;

use curl::{CurlConfig, Direction, TransferResult, Status};
use stats::DirectionStats;

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    name = "wasabi-speedtest-curl",
    about = "Wasabi S3 speed test via curl\n\n\
             Runs repeated uploads and/or downloads against a Wasabi S3 bucket,\n\
             measuring transfer time, throughput, and capturing diagnostic headers.\n\
             Requires curl >= 7.75 and AWS credentials (env vars or .env file).",
    after_help = "EXAMPLES:\n  \
        wasabi-speedtest-curl -b my-bucket --up -n 100\n  \
        wasabi-speedtest-curl -b my-bucket -n 50 --resolve-ip 38.27.106.131\n  \
        wasabi-speedtest-curl -b my-bucket --down -q --csv > results.csv",
)]
struct Cli {
    /// Upload only
    #[arg(long = "up", conflicts_with = "down")]
    up: bool,

    /// Download only
    #[arg(long = "down", conflicts_with = "up")]
    down: bool,

    /// Number of runs (0 = infinite)
    #[arg(short = 'n', default_value = "0")]
    count: u32,

    /// S3 bucket name (required)
    #[arg(short = 'b', long = "bucket")]
    bucket: String,

    /// Wasabi region
    #[arg(short = 'r', default_value = "us-east-1")]
    region: String,

    /// Quiet mode: only print non-OK results
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// Payload size in MB
    #[arg(short = 's', default_value = "10")]
    size_mb: u32,

    /// Curl --max-time in seconds
    #[arg(long = "timeout", default_value = "200")]
    timeout: u32,

    /// Seconds threshold for SLOW status
    #[arg(long = "slow", default_value = "2")]
    slow_secs: f64,

    /// Seconds threshold for CRAWL status
    #[arg(long = "crawl", default_value = "10")]
    crawl_secs: f64,

    /// Seconds threshold for STALL status
    #[arg(long = "stall", default_value = "30")]
    stall_secs: f64,

    /// Custom User-Agent string
    #[arg(short = 'A', long = "user-agent", default_value = "wasabi-speedtest-curl-rs/0.1")]
    user_agent: String,

    /// Pin all requests to a specific IP (uses curl --resolve)
    #[arg(long = "resolve-ip")]
    resolve_ip: Option<String>,

    /// Use HTTP instead of HTTPS
    #[arg(long = "insecure")]
    insecure: bool,

    /// CSV output (summary goes to stderr)
    #[arg(long = "csv")]
    csv: bool,
}

#[derive(Envconfig)]
struct EnvConfig {
    #[envconfig(from = "AWS_ACCESS_KEY_ID")]
    pub access_key: String,
    #[envconfig(from = "AWS_SECRET_ACCESS_KEY")]
    pub secret_key: String,
}

fn main() {
    let cli = Cli::parse();

    let use_color = !cli.csv && atty_stdout();

    // 1. Load .env
    dotenvy::dotenv().ok();

    // 2. Load credentials
    let env_cfg = match EnvConfig::init_from_env() {
        Ok(c) => c,
        Err(_) => {
            print_tag("[FAIL]", Color::Red, use_color);
            eprintln!(" AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY must be set (env or .env file)");
            std::process::exit(1);
        }
    };

    // 3. Check curl version
    match curl::parse_curl_version() {
        Ok((major, minor)) => {
            if major < 7 || (major == 7 && minor < 75) {
                print_tag("[FAIL]", Color::Red, use_color);
                eprintln!(" curl >= 7.75 required (for --aws-sigv4), found {major}.{minor}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            print_tag("[FAIL]", Color::Red, use_color);
            eprintln!(" {e}");
            std::process::exit(1);
        }
    }

    // 4. Resolve directions
    let (do_up, do_down) = match (cli.up, cli.down) {
        (true, false) => (true, false),
        (false, true) => (false, true),
        _ => (true, true),
    };

    // 5. Build endpoint
    let scheme = if cli.insecure { "http" } else { "https" };
    let endpoint = if cli.region == "us-east-1" {
        format!("{scheme}://s3.wasabisys.com")
    } else {
        format!("{scheme}://s3.{}.wasabisys.com", cli.region)
    };

    // Build --resolve arg: "hostname:port:ip"
    let resolve = cli.resolve_ip.as_ref().map(|ip| {
        let host = endpoint
            .strip_prefix("http://")
            .or_else(|| endpoint.strip_prefix("https://"))
            .unwrap_or(&endpoint);
        let port = if endpoint.starts_with("https://") { 443 } else { 80 };
        format!("{host}:{port}:{ip}")
    });

    if let Some(ref ip) = cli.resolve_ip {
        print_tag("[INFO]", Color::Cyan, use_color);
        eprintln!(" Pinning all requests to IP {ip}");
    }

    let cfg = CurlConfig {
        endpoint: endpoint.clone(),
        bucket: cli.bucket.clone(),
        region: cli.region.clone(),
        access_key: env_cfg.access_key,
        secret_key: env_cfg.secret_key,
        timeout: cli.timeout,
        user_agent: cli.user_agent.clone(),
        resolve,
        slow_secs: cli.slow_secs,
        crawl_secs: cli.crawl_secs,
        stall_secs: cli.stall_secs,
    };

    // 7. Check / create bucket
    print_tag("[INFO]", Color::Cyan, use_color);
    eprint!(" Checking bucket s3://{} ... ", cli.bucket);
    if curl::check_bucket(&cfg) {
        eprintln!("exists");
    } else {
        eprintln!("not found, creating...");
        if let Err(e) = curl::create_bucket(&cfg) {
            print_tag("[FAIL]", Color::Red, use_color);
            eprintln!(" Bucket creation failed: {e}");
            std::process::exit(1);
        }
        print_tag("[INFO]", Color::Cyan, use_color);
        eprintln!(" Bucket created");
    }

    // 8. Seed upload for download-only mode
    let seed_key = format!("speedtest-seed-{}MB.bin", cli.size_mb);
    if do_down && !do_up {
        print_tag("[INFO]", Color::Cyan, use_color);
        eprintln!(" Generating & uploading {}MB payload for download tests...", cli.size_mb);
        if let Err(e) = curl::seed_upload(&cfg, &seed_key, cli.size_mb) {
            print_tag("[FAIL]", Color::Red, use_color);
            eprintln!(" Seed upload failed: {e}");
            std::process::exit(1);
        }
    }

    // 9. Signal handler
    ctrlc::set_handler(|| {
        STOP.store(true, Ordering::SeqCst);
    })
    .expect("Failed to set Ctrl+C handler");

    // 10. Banner
    let mode_str = match (do_up, do_down) {
        (true, true) => "UP+DN",
        (true, false) => "UP only",
        (false, true) => "DN only",
        _ => unreachable!(),
    };
    let count_str = if cli.count == 0 {
        "infinite".to_string()
    } else {
        cli.count.to_string()
    };
    let quiet_str = if cli.quiet { " / quiet" } else { "" };

    if !cli.csv {
        let mut out = io::stdout();
        if use_color {
            let _ = out.execute(SetAttribute(Attribute::Bold));
        }
        print!(
            "Wasabi curl Speed Test — {}MB / {} / {} runs{}",
            cli.size_mb, mode_str, count_str, quiet_str
        );
        if use_color {
            let _ = out.execute(SetAttribute(Attribute::Reset));
        }
        println!(" (Ctrl+C to stop)");
        println!(
            "Endpoint: {}  Bucket: {}  Timeout: {}s",
            endpoint, cli.bucket, cli.timeout
        );
        println!("Started: {}", Local::now().format("%Y-%m-%d %H:%M:%S %Z"));
        print_header(do_up, do_down);
    } else {
        println!("run,id,timestamp,direction,time_s,speed_mb_s,bitrate,status,http_code,x-wasabi-cm-reference-id");
    }

    // 11. Main loop
    let run_date = Local::now().format("%Y%m%d%H%M%S").to_string();
    let script_start = std::time::Instant::now();
    let mut up_stats = DirectionStats::new();
    let mut dn_stats = DirectionStats::new();
    let mut run: u32 = 0;

    loop {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        run += 1;
        if cli.count > 0 && run > cli.count {
            break;
        }

        let rid = rand_hex();
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let obj_key = format!("speedtest-{run_date}-{run}-{rid}.bin");

        let mut up_result: Option<TransferResult> = None;
        let mut dn_result: Option<TransferResult> = None;

        // Upload
        if do_up {
            let r = curl::run_transfer(&cfg, Direction::Up, &obj_key, cli.size_mb);
            up_stats.record(&r);
            up_result = Some(r);
        }

        if STOP.load(Ordering::SeqCst) {
            // Still print this row, then break after
            print_row(
                &cli, use_color, do_up, do_down, run, &rid, &ts,
                up_result.as_ref(), dn_result.as_ref(),
            );
            break;
        }

        // Download
        if do_down {
            let dn_key = if do_up {
                obj_key.clone()
            } else {
                seed_key.clone()
            };
            let r = curl::run_transfer(&cfg, Direction::Down, &dn_key, cli.size_mb);
            dn_stats.record(&r);
            dn_result = Some(r);
        }

        // Print row
        print_row(
            &cli, use_color, do_up, do_down, run, &rid, &ts,
            up_result.as_ref(), dn_result.as_ref(),
        );

        if STOP.load(Ordering::SeqCst) {
            break;
        }
    }

    // 12. Summary
    let total_elapsed = script_start.elapsed();
    let total_mins = total_elapsed.as_secs() / 60;
    let total_secs = total_elapsed.as_secs() % 60;
    let total_runs = up_stats.total().max(dn_stats.total());

    if cli.csv {
        // Summary on stderr for CSV mode
        let stderr = &mut io::stderr();
        let _ = writeln!(stderr);
        let _ = writeln!(
            stderr,
            "Ended: {}  (ran {}m {}s)",
            Local::now().format("%Y-%m-%d %H:%M:%S %Z"),
            total_mins,
            total_secs
        );
        let _ = writeln!(stderr, "{total_runs} runs completed.");
        if do_up {
            print_direction_summary_plain(stderr, "Upload", &up_stats);
        }
        if do_down {
            print_direction_summary_plain(stderr, "Download", &dn_stats);
        }
    } else {
        println!();
        println!(
            "Ended: {}  (ran {}m {}s)",
            Local::now().format("%Y-%m-%d %H:%M:%S %Z"),
            total_mins,
            total_secs
        );
        println!("{total_runs} runs completed.");
        if do_up {
            print_direction_summary("Upload", &up_stats, use_color);
        }
        if do_down {
            print_direction_summary("Download", &dn_stats, use_color);
        }
    }
}

fn rand_hex() -> String {
    let bytes: [u8; 4] = rand::random();
    format!("{:02x}{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2], bytes[3])
}

fn atty_stdout() -> bool {
    crossterm::tty::IsTty::is_tty(&io::stdout())
}

fn print_tag(tag: &str, color: Color, use_color: bool) {
    if use_color {
        let mut out = io::stderr();
        let _ = out.execute(SetForegroundColor(color));
        let _ = out.execute(Print(tag));
        let _ = out.execute(ResetColor);
    } else {
        eprint!("{tag}");
    }
}

fn status_color(status: &Status) -> Color {
    match status {
        Status::Ok => Color::Green,
        Status::Slow => Color::Yellow,
        Status::Crawl => Color::Magenta,
        _ => Color::Red,
    }
}

fn print_header(do_up: bool, do_down: bool) {
    if do_up && do_down {
        println!();
        println!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} {:<5}  {:<10} {:>10}  {:<16} {:<5}",
            "Run", "ID", "Timestamp", "UP Time", "UP MB/s", "UP Bitrate", "UP",
            "DN Time", "DN MB/s", "DN Bitrate", "DN"
        );
        println!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} {:<5}  {:<10} {:>10}  {:<16} {:<5}",
            "---", "--------------", "-------------------", "-------", "-------", "----------", "--",
            "-------", "-------", "----------", "--"
        );
    } else {
        println!();
        println!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} {:<5}",
            "Run", "ID", "Timestamp", "Time", "MB/s", "Bitrate", "Status"
        );
        println!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} {:<5}",
            "---", "--------------", "-------------------", "-------", "-------", "----------", "------"
        );
    }
}

fn format_elapsed(result: &TransferResult) -> String {
    format!("{:.3}s", result.elapsed.as_secs_f64())
}

fn format_speed(result: &TransferResult) -> String {
    result
        .speed_mbs
        .map(|s| format!("{s:.2}"))
        .unwrap_or_else(|| "--".into())
}

fn format_bitrate_col(result: &TransferResult) -> String {
    result
        .bitrate
        .clone()
        .unwrap_or_else(|| "--".into())
}

fn print_row(
    cli: &Cli,
    use_color: bool,
    do_up: bool,
    do_down: bool,
    run: u32,
    rid: &str,
    ts: &str,
    up_result: Option<&TransferResult>,
    dn_result: Option<&TransferResult>,
) {
    // CSV mode
    if cli.csv {
        if let Some(r) = up_result {
            println!(
                "{},{}-{},{},up,{:.3},{},{},{},{},{}",
                run, run, rid, ts,
                r.elapsed.as_secs_f64(),
                r.speed_mbs.map(|s| format!("{s:.2}")).unwrap_or_default(),
                r.bitrate.as_deref().unwrap_or(""),
                r.status.label(),
                r.http_code,
                r.cm_ref_id.as_deref().unwrap_or(""),
            );
        }
        if let Some(r) = dn_result {
            println!(
                "{},{}-{},{},down,{:.3},{},{},{},{},{}",
                run, run, rid, ts,
                r.elapsed.as_secs_f64(),
                r.speed_mbs.map(|s| format!("{s:.2}")).unwrap_or_default(),
                r.bitrate.as_deref().unwrap_or(""),
                r.status.label(),
                r.http_code,
                r.cm_ref_id.as_deref().unwrap_or(""),
            );
        }
        return;
    }

    // Quiet mode filter
    if cli.quiet {
        let all_ok = match (up_result, dn_result) {
            (Some(u), Some(d)) => matches!(u.status, Status::Ok) && matches!(d.status, Status::Ok),
            (Some(u), None) => matches!(u.status, Status::Ok),
            (None, Some(d)) => matches!(d.status, Status::Ok),
            (None, None) => true,
        };
        if all_ok {
            return;
        }
    }

    let run_str = format!("#{run}");
    let id_str = format!("{run}-{rid}");

    if do_up && do_down {
        let (up_time, up_spd, up_br, up_st) = match up_result {
            Some(r) => (format_elapsed(r), format_speed(r), format_bitrate_col(r), r.status.clone()),
            None => ("--".into(), "--".into(), "--".into(), Status::Ok),
        };
        let (dn_time, dn_spd, dn_br, dn_st) = match dn_result {
            Some(r) => (format_elapsed(r), format_speed(r), format_bitrate_col(r), r.status.clone()),
            None => ("--".into(), "--".into(), "--".into(), Status::Ok),
        };

        print!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} ",
            run_str, id_str, ts, up_time, up_spd, up_br
        );
        print_colored_status(&up_st, use_color);
        print!(
            "  {:<10} {:>10}  {:<16} ",
            dn_time, dn_spd, dn_br
        );
        print_colored_status(&dn_st, use_color);
        println!();

        if let Some(r) = up_result {
            if !matches!(r.status, Status::Ok) {
                print_error_detail("UP", r, use_color);
            }
        }
        if let Some(r) = dn_result {
            if !matches!(r.status, Status::Ok) {
                print_error_detail("DN", r, use_color);
            }
        }
    } else {
        let (time, spd, br, st, result_ref) = if do_up {
            match up_result {
                Some(r) => (format_elapsed(r), format_speed(r), format_bitrate_col(r), r.status.clone(), Some(r)),
                None => ("--".into(), "--".into(), "--".into(), Status::Ok, None),
            }
        } else {
            match dn_result {
                Some(r) => (format_elapsed(r), format_speed(r), format_bitrate_col(r), r.status.clone(), Some(r)),
                None => ("--".into(), "--".into(), "--".into(), Status::Ok, None),
            }
        };

        print!(
            "  {:<6} {:<14} {:<19} {:<10} {:>10}  {:<16} ",
            run_str, id_str, ts, time, spd, br
        );
        print_colored_status(&st, use_color);
        println!();

        if let Some(r) = result_ref {
            if !matches!(r.status, Status::Ok) {
                let dir_label = if do_up { "UP" } else { "DN" };
                print_error_detail(dir_label, r, use_color);
            }
        }
    }
}

fn print_error_detail(dir: &str, result: &TransferResult, use_color: bool) {
    let cid = result.cm_ref_id.as_deref().unwrap_or("n/a");
    let ip = result.remote_ip.as_deref().unwrap_or("n/a");
    let line = format!(
        "  ^ {dir} http={} curl_exit={} remote_ip={ip} x-wasabi-cm-reference-id={cid}",
        result.http_code, result.curl_exit
    );
    if use_color {
        let mut out = io::stdout();
        let _ = out.execute(SetForegroundColor(Color::DarkRed));
        print!("{line}");
        let _ = out.execute(ResetColor);
    } else {
        print!("{line}");
    }
    println!();
}

fn print_colored_status(status: &Status, use_color: bool) {
    let label = status.label();
    if use_color {
        let mut out = io::stdout();
        let _ = out.execute(SetForegroundColor(status_color(status)));
        print!("{:<5}", label);
        let _ = out.execute(ResetColor);
    } else {
        print!("{:<5}", label);
    }
}

fn print_direction_summary(dir_name: &str, stats: &DirectionStats, use_color: bool) {
    let pad = if dir_name == "Upload" { "  " } else { "" };
    let mut out = io::stdout();

    print!("{dir_name}:{pad} ");
    if use_color {
        let _ = out.execute(SetForegroundColor(Color::Green));
        print!("{} ok", stats.ok);
        let _ = out.execute(ResetColor);
        print!("  ");
        let _ = out.execute(SetForegroundColor(Color::Yellow));
        print!("{} slow", stats.slow);
        let _ = out.execute(ResetColor);
        print!("  ");
        let _ = out.execute(SetForegroundColor(Color::Magenta));
        print!("{} crawl", stats.crawl);
        let _ = out.execute(ResetColor);
        print!("  ");
        let _ = out.execute(SetForegroundColor(Color::Red));
        print!(
            "{} stall  {} tmout  {} reset  {} err",
            stats.stall, stats.tmout, stats.reset, stats.err
        );
        let _ = out.execute(ResetColor);
    } else {
        print!(
            "{} ok  {} slow  {} crawl  {} stall  {} tmout  {} reset  {} err",
            stats.ok, stats.slow, stats.crawl, stats.stall, stats.tmout, stats.reset, stats.err
        );
    }
    println!("  ({}% success)", stats.success_pct());

    if let (Some(avg_t), Some(avg_s)) = (stats.avg_time(), stats.avg_speed()) {
        let bitrate = curl::format_bitrate(avg_s);
        println!(
            "          avg: {:.3}s  {:.2} MB/s  {}  ({} successful transfers)",
            avg_t,
            avg_s,
            bitrate,
            stats.success_count()
        );
    }

    if let Some(p) = stats.percentiles() {
        println!(
            "          min: {:.3}s  max: {:.3}s  p50: {:.3}s  p95: {:.3}s  p99: {:.3}s",
            p.min, p.max, p.p50, p.p95, p.p99
        );
    }
}

fn print_direction_summary_plain(w: &mut impl Write, dir_name: &str, stats: &DirectionStats) {
    let pad = if dir_name == "Upload" { "  " } else { "" };
    let _ = writeln!(
        w,
        "{dir_name}:{pad} {} ok  {} slow  {} crawl  {} stall  {} tmout  {} reset  {} err  ({}% success)",
        stats.ok, stats.slow, stats.crawl, stats.stall, stats.tmout, stats.reset, stats.err,
        stats.success_pct()
    );
    if let (Some(avg_t), Some(avg_s)) = (stats.avg_time(), stats.avg_speed()) {
        let bitrate = curl::format_bitrate(avg_s);
        let _ = writeln!(
            w,
            "          avg: {:.3}s  {:.2} MB/s  {}  ({} successful transfers)",
            avg_t, avg_s, bitrate, stats.success_count()
        );
    }
    if let Some(p) = stats.percentiles() {
        let _ = writeln!(
            w,
            "          min: {:.3}s  max: {:.3}s  p50: {:.3}s  p95: {:.3}s  p99: {:.3}s",
            p.min, p.max, p.p50, p.p95, p.p99
        );
    }
}
