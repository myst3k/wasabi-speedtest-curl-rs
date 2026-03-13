# wasabi-speedtest-curl

Wasabi S3 connectivity diagnostic tool. Runs repeated uploads and/or downloads against a Wasabi bucket, measuring transfer time, throughput, and capturing diagnostic response headers.

Single static binary, no runtime dependencies beyond `curl >= 7.75`.

## Why curl?

The tool intentionally shells out to `curl` for transfers (rather than using an HTTP library) so transfer behavior is identical to what users experience and is diagnosable with pcap. Uploads pipe random data to `curl -T -` from memory — no test file needed.

### Reference curl commands

**Upload:**
```bash
curl -s -o /dev/null \
  -w "%{http_code}\n%{remote_ip}" \
  -D /dev/stderr \
  -A "wasabi-speedtest-curl-rs/0.1" \
  --aws-sigv4 "aws:amz:us-east-1:s3" \
  --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" \
  --max-time 200 \
  -X PUT -T - \
  "https://s3.wasabisys.com/BUCKET/speedtest-YYYYMMDDHHMMSS-N-RAND.bin"
```

**Download:**
```bash
curl -s -o /dev/null \
  -w "%{http_code}\n%{remote_ip}" \
  -D /dev/stderr \
  -A "wasabi-speedtest-curl-rs/0.1" \
  --aws-sigv4 "aws:amz:us-east-1:s3" \
  --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" \
  --max-time 200 \
  "https://s3.wasabisys.com/BUCKET/speedtest-YYYYMMDDHHMMSS-N-RAND.bin"
```

## Install

```bash
cargo build --release
# Binary at: target/release/wasabi-speedtest-curl
```

### Cross-compile for Linux (from macOS)

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

## Usage

```
wasabi-speedtest-curl [OPTIONS] --bucket <BUCKET>
```

### Options

```
      --up                       Upload only
      --down                     Download only
  -n <COUNT>                     Number of runs (0 = infinite) [default: 0]
  -b, --bucket <BUCKET>          S3 bucket name (required)
  -r <REGION>                    Wasabi region [default: us-east-1]
  -q, --quiet                    Quiet mode: only print non-OK results
  -s <SIZE_MB>                   Payload size in MB [default: 10]
      --timeout <TIMEOUT>        Curl --max-time in seconds [default: 200]
      --slow <SLOW_SECS>         Seconds threshold for SLOW status [default: 2]
      --crawl <CRAWL_SECS>       Seconds threshold for CRAWL status [default: 10]
      --stall <STALL_SECS>       Seconds threshold for STALL status [default: 30]
  -A, --user-agent <USER_AGENT>  Custom User-Agent string [default: wasabi-speedtest-curl-rs/0.1]
      --resolve-ip <RESOLVE_IP>  Pin all requests to a specific IP (uses curl --resolve)
      --insecure                 Use HTTP instead of HTTPS
      --csv                      CSV output (summary goes to stderr)
```

### Examples

```bash
# Upload-only test, 100 runs
wasabi-speedtest-curl -b my-bucket --up -n 100

# Pin to a specific IP to isolate issues
wasabi-speedtest-curl -b my-bucket -n 50 --resolve-ip 38.27.106.131

# Download test, quiet mode, CSV to file
wasabi-speedtest-curl -b my-bucket --down -q --csv > results.csv

# Larger payload with adjusted thresholds
wasabi-speedtest-curl -b my-bucket -s 100 --slow 5 --crawl 25 --stall 75
```

## Output

### Normal mode

```
Wasabi curl Speed Test — 10MB / UP only / 500 runs / quiet (Ctrl+C to stop)
Endpoint: https://s3.wasabisys.com  Bucket: wasabi-speedtest-diag-rs  Timeout: 200s
Started: 2026-03-12 13:57:35 -04:00

  Run    ID             Timestamp           Time             MB/s  Bitrate          Status
  ---    -------------- ------------------- -------       -------  ----------       ------
  #81    81-ac2520a8    2026-03-12 13:58:32 43.532s            --  --               RESET
  ^ UP http=0 curl_exit=56 remote_ip=38.27.106.104 x-wasabi-cm-reference-id=n/a
  #112   112-8a141c3b   2026-03-12 13:59:37 49.190s          0.20  1.6 Mbps         STALL
  ^ UP http=200 curl_exit=0 remote_ip=38.27.106.122 x-wasabi-cm-reference-id=1773338377 B14-U38.prod1.ashburn 1768249887:17014161:71
```

Non-OK results (SLOW, CRAWL, STALL, TMOUT, RESET, errors) print a detail line with:
- HTTP response code
- curl exit code (56=recv error, 28=timeout, 7=connection refused)
- Remote IP that curl connected to
- `x-wasabi-cm-reference-id` header for server-side correlation

### Summary

```
500 runs completed.
Upload:   487 ok  1 slow  2 crawl  3 stall  0 tmout  7 reset  0 err  (98% success)
          avg: 1.019s  14.11 MB/s  112.9 Mbps  (493 successful transfers)
          min: 0.537s  max: 49.190s  p50: 0.702s  p95: 0.843s  p99: 5.256s
```

### Status classification

| Status | Condition |
|--------|-----------|
| OK | Transfer completed under `--slow` threshold |
| SLOW | Transfer time >= `--slow` (default 2s) |
| CRAWL | Transfer time >= `--crawl` (default 10s) |
| STALL | Transfer time >= `--stall` (default 30s) |
| TMOUT | curl exit 28 (--max-time exceeded) |
| RESET | curl exit 56 (connection reset during transfer) |
| RFUSD | curl exit 7 (connection refused) |
| H{code} | HTTP error (e.g. H403, H500) |
| ERR{code} | Other curl exit code |
