use crate::curl::TransferResult;

pub struct DirectionStats {
    pub ok: u32,
    pub slow: u32,
    pub crawl: u32,
    pub stall: u32,
    pub tmout: u32,
    pub reset: u32,
    pub skipped: u32,
    pub err: u32,
    pub times: Vec<f64>,
    pub speeds: Vec<f64>,
}

impl DirectionStats {
    pub fn new() -> Self {
        Self {
            ok: 0,
            slow: 0,
            crawl: 0,
            stall: 0,
            tmout: 0,
            reset: 0,
            skipped: 0,
            err: 0,
            times: Vec::new(),
            speeds: Vec::new(),
        }
    }

    pub fn record(&mut self, result: &TransferResult) {
        use crate::curl::Status;
        match &result.status {
            Status::Ok => self.ok += 1,
            Status::Slow => self.slow += 1,
            Status::Crawl => self.crawl += 1,
            Status::Stall => self.stall += 1,
            Status::Tmout => self.tmout += 1,
            Status::Reset => self.reset += 1,
            Status::Skipped => self.skipped += 1,
            Status::Rfusd | Status::HttpErr(_) | Status::CurlErr(_) => self.err += 1,
        }
        if result.status.is_success() {
            self.times.push(result.elapsed.as_secs_f64());
            if let Some(speed) = result.speed_mbs {
                self.speeds.push(speed);
            }
        }
    }

    pub fn total(&self) -> u32 {
        self.ok + self.slow + self.crawl + self.stall + self.tmout + self.reset + self.skipped + self.err
    }

    pub fn success_count(&self) -> u32 {
        self.ok + self.slow + self.crawl + self.stall
    }

    pub fn success_pct(&self) -> u32 {
        let t = self.total();
        if t == 0 { 0 } else { self.success_count() * 100 / t }
    }

    pub fn avg_time(&self) -> Option<f64> {
        if self.times.is_empty() {
            None
        } else {
            Some(self.times.iter().sum::<f64>() / self.times.len() as f64)
        }
    }

    pub fn avg_speed(&self) -> Option<f64> {
        if self.speeds.is_empty() {
            None
        } else {
            Some(self.speeds.iter().sum::<f64>() / self.speeds.len() as f64)
        }
    }

    pub fn percentiles(&self) -> Option<Percentiles> {
        if self.times.is_empty() {
            return None;
        }
        let mut sorted = self.times.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = sorted.len();
        Some(Percentiles {
            min: sorted[0],
            max: sorted[n - 1],
            p50: percentile_val(&sorted, 50.0),
            p95: percentile_val(&sorted, 95.0),
            p99: percentile_val(&sorted, 99.0),
        })
    }

    pub fn merge(&mut self, other: DirectionStats) {
        self.ok += other.ok;
        self.slow += other.slow;
        self.crawl += other.crawl;
        self.stall += other.stall;
        self.tmout += other.tmout;
        self.reset += other.reset;
        self.skipped += other.skipped;
        self.err += other.err;
        self.times.extend(other.times);
        self.speeds.extend(other.speeds);
    }
}

pub struct Percentiles {
    pub min: f64,
    pub max: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

fn percentile_val(sorted: &[f64], pct: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = pct / 100.0 * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let frac = rank - lower as f64;
        sorted[lower] * (1.0 - frac) + sorted[upper] * frac
    }
}
