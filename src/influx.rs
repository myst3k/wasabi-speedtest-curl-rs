use crate::curl::{Direction, TransferResult};
use crate::rolling::Deviation;
use chrono::Utc;

pub struct InfluxConfig {
    pub url: String,
    pub token: String,
    pub org: String,
    pub bucket: String,
}

impl InfluxConfig {
    pub fn write_point(
        &self,
        result: &TransferResult,
        direction: Direction,
        region: &str,
        size_bytes: u64,
        instance: &str,
        provider: &str,
        geo: Option<&str>,
        zone: Option<&str>,
        deviation: Option<&Deviation>,
    ) {
        let line = format_line_protocol(result, direction, region, size_bytes, instance, provider, geo, zone, deviation);
        let url = format!(
            "{}/api/v2/write?org={}&bucket={}&precision=ns",
            self.url, self.org, self.bucket
        );

        let resp = ureq::post(&url)
            .header("Authorization", &format!("Token {}", self.token))
            .content_type("text/plain")
            .send(line.as_bytes());

        if let Err(e) = resp {
            eprintln!("[WARN] InfluxDB write failed: {e}");
        }
    }
}

fn format_line_protocol(
    result: &TransferResult,
    direction: Direction,
    region: &str,
    size_bytes: u64,
    instance: &str,
    provider: &str,
    geo: Option<&str>,
    zone: Option<&str>,
    deviation: Option<&Deviation>,
) -> String {
    let dir_tag = match direction {
        Direction::Up => "upload",
        Direction::Down => "download",
    };

    let t = &result.timings;
    let speed_mbs = result.speed_mbs.unwrap_or(0.0);
    let status = result.status.label();
    let remote_ip = result.remote_ip.as_deref().unwrap_or("");
    let ts_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0);

    let mut tags = format!(
        "s3_transfer,instance={instance},provider={provider},direction={dir_tag},region={region}"
    );
    if let Some(g) = geo {
        tags.push_str(&format!(",geo={g}"));
    }
    if let Some(z) = zone {
        tags.push_str(&format!(",zone={z}"));
    }

    let mut fields = format!(
        "status=\"{status}\",\
         remote_ip=\"{remote_ip}\",\
         elapsed={:.6},\
         speed_mbs={speed_mbs:.4},\
         size_bytes={size_bytes}i,\
         http_code={}i,\
         dns={:.6},\
         tcp_connect={:.6},\
         tls_handshake={:.6},\
         server_processing={:.6},\
         data_transfer={:.6},\
         ttfb={:.6},\
         time_total={:.6},\
         speed_download={:.0},\
         speed_upload={:.0},\
         size_download={}i,\
         size_upload={}i,\
         num_connects={}i",
        result.elapsed.as_secs_f64(),
        result.http_code,
        t.dns(),
        t.tcp_connect(),
        t.tls_handshake(),
        t.server_processing(),
        t.data_transfer(),
        t.time_starttransfer,
        t.time_total,
        t.speed_download,
        t.speed_upload,
        t.size_download,
        t.size_upload,
        t.num_connects,
    );

    if let Some(dev) = deviation {
        fields.push_str(&format!(
            ",deviation=\"{}\",zscore={:.3},rolling_mean={:.6},rolling_stddev={:.6}",
            dev.label, dev.zscore, dev.mean, dev.stddev
        ));
    }

    format!("{tags} {fields} {ts_ns}")
}
