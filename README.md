# wasabi-speedtest-curl

S3 connectivity diagnostic and performance comparison tool. Loops concurrent curl uploads and/or downloads against any S3-compatible endpoint — indefinitely or for a set number of runs. Generates random payloads in memory (configurable size, optional log-uniform randomization). Measures transfer time, throughput, connection timing breakdown, and captures diagnostic response headers. Supports multi-threaded operation and InfluxDB integration for long-term monitoring and cross-provider comparison.

Single static binary, no runtime dependencies beyond `curl >= 7.75`.

## Why curl?

The tool intentionally shells out to `curl` for transfers (rather than using an HTTP library) so transfer behavior is identical to what users experience and is diagnosable with pcap. Uploads pipe random data to `curl -T -` from memory — no test file needed.

## Install

**Linux (amd64):**
```bash
curl -sL https://github.com/myst3k/wasabi-speedtest-curl-rs/releases/latest/download/wasabi-speedtest-curl-linux-amd64.tar.gz | tar xz
```

**macOS (Apple Silicon):**
```bash
curl -sL https://github.com/myst3k/wasabi-speedtest-curl-rs/releases/latest/download/wasabi-speedtest-curl-macos-arm64.tar.gz | tar xz
```

**macOS (Intel):**
```bash
curl -sL https://github.com/myst3k/wasabi-speedtest-curl-rs/releases/latest/download/wasabi-speedtest-curl-macos-amd64.tar.gz | tar xz
```

Or build from source:

```bash
cargo build --release
# Binary at: target/release/wasabi-speedtest-curl
```

**Cross-compile for Linux (from macOS):**
```bash
cross build --release --target x86_64-unknown-linux-gnu
scp target/x86_64-unknown-linux-gnu/release/wasabi-speedtest-curl user@host:~/
```

## Credentials

Set via environment variables or a `.env` file next to the binary:

```
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
```

Region resolution order: `-r` flag > `AWS_REGION` env > `AWS_DEFAULT_REGION` env > `us-east-1`

## Usage

```
wasabi-speedtest-curl [OPTIONS] --bucket <BUCKET>
```

### Options

```
  -b, --bucket <BUCKET>          S3 bucket name (required)
  -r <REGION>                    S3 region for SigV4 signing [default: us-east-1]
      --endpoint <URL>           Custom S3 endpoint URL (overrides default Wasabi endpoint)
  -n <COUNT>                     Number of runs (0 = infinite) [default: 0]
  -s <SIZE_MB>                   Payload size in MB [default: 10]
      --randomize                Randomize object size between 4KB and -s (log-uniform)
  -t, --threads <N>              Number of concurrent transfer threads [default: 1]
      --up                       Upload only
      --down                     Download only
  -q, --quiet                    Quiet mode: only print non-OK results
      --timeout <TIMEOUT>        Curl --max-time in seconds [default: 200]
      --slow <SLOW_SECS>         Seconds threshold for SLOW status [default: 2]
      --crawl <CRAWL_SECS>       Seconds threshold for CRAWL status [default: 10]
      --stall <STALL_SECS>       Seconds threshold for STALL status [default: 30]
  -A, --user-agent <USER_AGENT>  Custom User-Agent string [default: wasabi-speedtest-curl-rs/0.1]
      --resolve-ip <RESOLVE_IP>  Pin all requests to a specific IP (uses curl --resolve)
      --insecure                 Use HTTP instead of HTTPS
      --csv                      CSV output (summary goes to stderr)
      --instance <NAME>          Instance name for identifying this probe [default: hostname-based]
      --provider <NAME>          Storage provider name [default: auto-detected from endpoint]
      --geo <GEO>                Geographic market (e.g. NA, EMEA, LATAM, APAC)
      --zone <ZONE>              Provider-agnostic zone (e.g. us-east, eu-west, ap-southeast)
      --influxdb-url <URL>       InfluxDB URL (e.g. http://localhost:8086)
      --influxdb-token <TOKEN>   InfluxDB API token
      --influxdb-org <ORG>       InfluxDB organization [default: wasabi]
      --influxdb-bucket <BUCKET> InfluxDB bucket [default: s3_diagnostics]
```

### Examples

```bash
# Basic test against Wasabi us-east-1
wasabi-speedtest-curl -b my-bucket -n 20

# 4 concurrent threads, random object sizes up to 100MB
wasabi-speedtest-curl -b my-bucket -s 100 --randomize -t 4

# Test against Cloudflare R2 (auto-detects provider)
wasabi-speedtest-curl -b my-r2-bucket \
  --endpoint https://abc123.r2.cloudflarestorage.com \
  --zone us-east --geo NA

# Continuous monitoring with InfluxDB
wasabi-speedtest-curl -b my-bucket \
  --instance probe-nyc-01 \
  --influxdb-url http://influxdb:8086 \
  --influxdb-token mytoken \
  --influxdb-bucket s3_diagnostics

# Pin to a specific IP to isolate issues
wasabi-speedtest-curl -b my-bucket -n 50 --resolve-ip 38.27.106.131

# CSV output for analysis
wasabi-speedtest-curl -b my-bucket --csv > results.csv
```

## Output

### Normal mode

```
Wasabi curl Speed Test — 10MB / UP+DN / 20 runs (Ctrl+C to stop)
Endpoint: https://s3.wasabisys.com  Bucket: my-bucket  Timeout: 200s
Started: 2026-03-18 15:19:59 -04:00

  Run    ID             Timestamp               Size | UP Time       UP MB/s  UP Bitrate       UP Status      | DN Time       DN MB/s  DN Bitrate       DN Status
  ---    -------------- ------------------- -------- | -------       -------  ----------       ---------      | -------       -------  ----------       ---------
  #1     1-8dd2a7a3     2026-03-18 15:19:59   10.0MB | 0.810s          12.35  98.8 Mbps        OK             | 0.547s          18.27  146.2 Mbps       OK
  #2     2-4ef679ed     2026-03-18 15:20:01   10.0MB | 0.818s          12.22  97.8 Mbps        OK             | 0.665s          15.05  120.4 Mbps       OK
  #13    13-6a0d8ddd    2026-03-18 15:21:43   10.0MB | 1.168s           8.56  68.5 Mbps        OK ↑↑2.6σ      | 0.863s          11.59  92.7 Mbps        OK
```

Non-OK results print a detail line with HTTP code, curl exit code, remote IP, and `x-wasabi-cm-reference-id` for server-side correlation.

### Status classification

Two independent classification systems:

**Fixed thresholds (speed class):**

| Status | Condition |
|--------|-----------|
| OK | Transfer completed under `--slow` threshold |
| SLOW | Transfer time >= `--slow` (default 2s) |
| CRAWL | Transfer time >= `--crawl` (default 10s) |
| STALL | Transfer time >= `--stall` (default 30s) |
| TMOUT | curl exit 28 (--max-time exceeded) |
| RESET | curl exit 56 (connection reset during transfer) |
| RFUSD | curl exit 7 (connection refused) |
| SKIP | Download skipped because upload failed |
| H{code} | HTTP error (e.g. H403, H500) |
| ERR{code} | Other curl exit code |

**Rolling deviation (relative to recent history):**

After 10 OK transfers, each new OK transfer is compared against the rolling window (last 100 OK transfers) using standard deviation:

| Indicator | Condition | Color |
|-----------|-----------|-------|
| *(none)* | Within 1σ of mean (NORMAL) | |
| ↑1.5σ | 1-2σ above mean (ELEVATED) | Yellow |
| ↑↑2.3σ | 2-3σ above mean (HIGH) | Magenta |
| ↑↑↑4.1σ | 3+σ above mean (OUTLIER) | Red |

Deviation is skipped when `--randomize` is active since object sizes vary.

### Summary

```
20 runs completed.
Upload:   15 ok  5 slow  0 crawl  0 stall  0 tmout  0 reset  0 skip  0 err  (100% success)
          avg: 1.394s  8.92 MB/s  71.4 Mbps  (20 successful transfers)
          min: 0.711s  max: 3.076s  p50: 1.014s  p95: 2.668s  p99: 2.995s
Download: 11 ok  1 slow  4 crawl  4 stall  0 tmout  0 reset  0 skip  0 err  (100% success)
          avg: 15.623s  7.39 MB/s  59.1 Mbps  (20 successful transfers)
          min: 0.482s  max: 75.987s  p50: 1.348s  p95: 55.575s  p99: 71.905s
```

## Curl timing breakdown

Each transfer captures detailed curl timing data (available in CSV and InfluxDB):

| Metric | Description |
|--------|-------------|
| dns | DNS resolution time |
| tcp_connect | TCP handshake time |
| tls_handshake | TLS negotiation time |
| server_processing | Server-side latency (TTFB minus connection setup) |
| data_transfer | Actual data transfer time |
| ttfb | Time to first byte |
| time_total | Total request time |

## InfluxDB integration

Data is written to InfluxDB v2 using line protocol. Each transfer creates a data point with:

**Tags (for filtering/grouping):**
- `instance` — probe identifier
- `provider` — storage provider (auto-detected or manual)
- `direction` — upload/download
- `region` — provider-specific region
- `geo` — geographic market (NA, EMEA, etc.)
- `zone` — provider-agnostic zone (us-east, eu-west, etc.)

**Fields:**
- All timing metrics (elapsed, dns, tcp_connect, tls_handshake, server_processing, data_transfer, ttfb, time_total)
- Transfer stats (speed_mbs, size_bytes, http_code, speed_download, speed_upload)
- Status and deviation (status, deviation, zscore, rolling_mean, rolling_stddev)
- Connection info (remote_ip, num_connects)

## Multi-provider comparison

Run multiple probes against different providers, all writing to the same InfluxDB:

```bash
# Wasabi (auto-detects provider, zone, geo)
wasabi-speedtest-curl -b wasabi-bucket --instance probe-nyc-01 --influxdb-url ...

# Cloudflare R2 (auto-detects provider from endpoint)
wasabi-speedtest-curl -b r2-bucket \
  --endpoint https://abc123.r2.cloudflarestorage.com \
  --zone us-east --geo NA \
  --instance probe-nyc-01 --influxdb-url ...

# AWS S3 (auto-detects provider from endpoint)
wasabi-speedtest-curl -b aws-bucket \
  --endpoint https://s3.us-east-1.amazonaws.com \
  -r us-east-1 \
  --instance probe-nyc-01 --influxdb-url ...

# On-prem S3-compatible (e.g. RustFS, MinIO) — use --provider for unrecognized endpoints
wasabi-speedtest-curl -b test-bucket \
  --endpoint https://s3.internal.example.com \
  --provider rustfs --zone us-east --geo NA \
  --instance probe-nyc-01 --influxdb-url ...
```

Dashboard groups by `provider` tag for side-by-side comparison of throughput, TTFB, and error rates.
