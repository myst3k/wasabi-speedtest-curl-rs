use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct CurlConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub timeout: u32,
    pub user_agent: String,
    pub resolve: Option<String>,
    pub slow_secs: f64,
    pub crawl_secs: f64,
    pub stall_secs: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Clone)]
pub enum Status {
    Ok,
    Slow,
    Crawl,
    Stall,
    Tmout,
    Reset,
    Rfusd,
    HttpErr(u16),
    CurlErr(i32),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Ok => "OK".into(),
            Status::Slow => "SLOW".into(),
            Status::Crawl => "CRAWL".into(),
            Status::Stall => "STALL".into(),
            Status::Tmout => "TMOUT".into(),
            Status::Reset => "RESET".into(),
            Status::Rfusd => "RFUSD".into(),
            Status::HttpErr(code) => format!("H{code}"),
            Status::CurlErr(code) => format!("ERR{code}"),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Status::Ok | Status::Slow | Status::Crawl | Status::Stall)
    }
}

pub struct TransferResult {
    pub status: Status,
    pub http_code: u16,
    pub curl_exit: i32,
    pub remote_ip: Option<String>,
    pub elapsed: Duration,
    pub speed_mbs: Option<f64>,
    pub bitrate: Option<String>,
    pub cm_ref_id: Option<String>,
}

pub fn format_bitrate(mb_per_sec: f64) -> String {
    let mbps = mb_per_sec * 8.0;
    if mbps >= 1000.0 {
        format!("{:.2} Gbps", mbps / 1000.0)
    } else {
        format!("{:.1} Mbps", mbps)
    }
}

pub fn check_bucket(cfg: &CurlConfig) -> bool {
    let url = format!("{}/{}", cfg.endpoint, cfg.bucket);
    let sigv4 = format!("aws:amz:{}:s3", cfg.region);
    let user = format!("{}:{}", cfg.access_key, cfg.secret_key);
    let mut args = vec![
        "-s", "-o", "/dev/null", "-w", "%{http_code}",
        "-A", &cfg.user_agent,
        "-I",
        "--aws-sigv4", &sigv4,
        "--user", &user,
    ];
    if let Some(ref r) = cfg.resolve {
        args.extend_from_slice(&["--resolve", r]);
    }
    args.push(&url);

    match Command::new("curl").args(&args).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "200",
        Err(_) => false,
    }
}

pub fn create_bucket(cfg: &CurlConfig) -> Result<(), String> {
    let url = format!("{}/{}", cfg.endpoint, cfg.bucket);
    let sigv4 = format!("aws:amz:{}:s3", cfg.region);
    let user = format!("{}:{}", cfg.access_key, cfg.secret_key);
    let mut args = vec![
        "-s", "-o", "/dev/null", "-w", "%{http_code}",
        "-A", &cfg.user_agent,
        "-X", "PUT",
        "--aws-sigv4", &sigv4,
        "--user", &user,
    ];
    if let Some(ref r) = cfg.resolve {
        args.extend_from_slice(&["--resolve", r]);
    }
    args.push(&url);

    let output = Command::new("curl").args(&args).output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;
    let code = String::from_utf8_lossy(&output.stdout);
    if code.trim() == "200" {
        Ok(())
    } else {
        Err(format!("Bucket creation returned HTTP {}", code.trim()))
    }
}

pub fn seed_upload(cfg: &CurlConfig, obj_key: &str, size_mb: u32) -> Result<(), String> {
    let url = format!("{}/{}/{}", cfg.endpoint, cfg.bucket, obj_key);
    let sigv4 = format!("aws:amz:{}:s3", cfg.region);
    let user = format!("{}:{}", cfg.access_key, cfg.secret_key);
    let timeout_str = cfg.timeout.to_string();
    let mut args = vec![
        "-s", "-o", "/dev/null", "-w", "%{http_code}",
        "-A", &cfg.user_agent,
        "--aws-sigv4", &sigv4,
        "--user", &user,
        "--max-time", &timeout_str,
        "-X", "PUT", "-T", "-",
    ];
    if let Some(ref r) = cfg.resolve {
        args.extend_from_slice(&["--resolve", r]);
    }
    args.push(&url);

    let mut child = Command::new("curl")
        .args(&args)
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run curl: {e}"))?;

    feed_random_stdin(child.stdin.take().unwrap(), size_mb);

    let output = child.wait_with_output().map_err(|e| format!("curl wait failed: {e}"))?;
    let code = String::from_utf8_lossy(&output.stdout);
    if code.trim() == "200" {
        Ok(())
    } else {
        Err(format!("Seed upload returned HTTP {}", code.trim()))
    }
}

pub fn run_transfer(
    cfg: &CurlConfig,
    direction: Direction,
    obj_key: &str,
    size_mb: u32,
) -> TransferResult {
    let url = format!("{}/{}/{}", cfg.endpoint, cfg.bucket, obj_key);

    let sigv4 = format!("aws:amz:{}:s3", cfg.region);
    let user = format!("{}:{}", cfg.access_key, cfg.secret_key);
    let timeout_str = cfg.timeout.to_string();

    let mut args: Vec<&str> = vec![
        "-s", "-o", "/dev/null",
        "-w", "%{http_code}\n%{remote_ip}",
        "-D", "/dev/stderr",
        "-A", &cfg.user_agent,
        "--aws-sigv4", &sigv4,
        "--user", &user,
        "--max-time", &timeout_str,
    ];

    if direction == Direction::Up {
        args.extend_from_slice(&["-X", "PUT", "-T", "-"]);
    }
    if let Some(ref r) = cfg.resolve {
        args.extend_from_slice(&["--resolve", r]);
    }

    args.push(&url);

    let needs_stdin = direction == Direction::Up;

    let start = Instant::now();

    let spawn_result = Command::new("curl")
        .args(&args)
        .stdin(if needs_stdin { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();

    let (exit_code, http_code, remote_ip, cm_ref_id, elapsed) = match spawn_result {
        Ok(mut child) => {
            if needs_stdin {
                feed_random_stdin(child.stdin.take().unwrap(), size_mb);
            }
            match child.wait_with_output() {
                Ok(out) => {
                    let elapsed = start.elapsed();
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let mut lines = stdout.lines();
                    let http: u16 = lines.next().unwrap_or("0").trim().parse().unwrap_or(0);
                    let ip = lines.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                    let exit = out.status.code().unwrap_or(-1);
                    let headers = String::from_utf8_lossy(&out.stderr);
                    let cid = parse_cm_ref_id(&headers);
                    (exit, http, ip, cid, elapsed)
                }
                Err(_) => (-1, 0u16, None, None, start.elapsed()),
            }
        }
        Err(_) => (-1, 0u16, None, None, start.elapsed()),
    };

    let status = classify(exit_code, http_code, &elapsed, cfg);
    let (speed_mbs, bitrate) = if status.is_success() && http_code == 200 {
        let secs = elapsed.as_secs_f64();
        if secs > 0.0 {
            let speed = size_mb as f64 / secs;
            let br = format_bitrate(speed);
            (Some(speed), Some(br))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    TransferResult {
        status,
        http_code,
        curl_exit: exit_code,
        remote_ip,
        elapsed,
        speed_mbs,
        bitrate,
        cm_ref_id,
    }
}

fn parse_cm_ref_id(headers: &str) -> Option<String> {
    for line in headers.lines() {
        if let Some(val) = line.strip_prefix("x-wasabi-cm-reference-id:") {
            return Some(val.trim().to_string());
        }
        if let Some(val) = line.strip_prefix("X-Wasabi-Cm-Reference-Id:") {
            return Some(val.trim().to_string());
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("x-wasabi-cm-reference-id:") {
            return Some(line[25..].trim().to_string());
        }
    }
    None
}

fn feed_random_stdin(stdin: std::process::ChildStdin, size_mb: u32) {
    let total_bytes = size_mb as usize * 1024 * 1024;
    std::thread::spawn(move || {
        let mut writer = std::io::BufWriter::new(stdin);
        let mut remaining = total_bytes;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let chunk = remaining.min(buf.len());
            rand::fill(&mut buf[..chunk]);
            if writer.write_all(&buf[..chunk]).is_err() {
                break;
            }
            remaining -= chunk;
        }
    });
}

fn classify(exit_code: i32, http_code: u16, elapsed: &Duration, cfg: &CurlConfig) -> Status {
    if exit_code == 28 {
        return Status::Tmout;
    }
    if exit_code == 56 {
        return Status::Reset;
    }
    if exit_code == 7 {
        return Status::Rfusd;
    }
    if exit_code != 0 && http_code == 0 {
        return Status::CurlErr(exit_code);
    }
    if http_code != 200 {
        return Status::HttpErr(http_code);
    }

    let secs = elapsed.as_secs_f64();
    if secs >= cfg.stall_secs {
        Status::Stall
    } else if secs >= cfg.crawl_secs {
        Status::Crawl
    } else if secs >= cfg.slow_secs {
        Status::Slow
    } else {
        Status::Ok
    }
}

pub fn parse_curl_version() -> Result<(u32, u32), String> {
    let output = Command::new("curl")
        .arg("--version")
        .output()
        .map_err(|_| "curl not found in PATH".to_string())?;

    let text = String::from_utf8_lossy(&output.stdout);
    let first_line = text.lines().next().unwrap_or("");

    // "curl 8.7.1 ..."
    for word in first_line.split_whitespace() {
        let parts: Vec<&str> = word.split('.').collect();
        if parts.len() >= 2 {
            if let (Ok(major), Ok(minor)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                return Ok((major, minor));
            }
        }
    }
    Err(format!("Could not parse curl version from: {first_line}"))
}
