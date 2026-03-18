use std::collections::VecDeque;

pub struct RollingStats {
    window: VecDeque<f64>,
    capacity: usize,
}

pub struct Deviation {
    pub label: &'static str,
    pub zscore: f64,
    pub mean: f64,
    pub stddev: f64,
}

impl RollingStats {
    pub fn new(capacity: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.window.len() >= 10
    }

    pub fn classify(&mut self, value: f64) -> Deviation {
        let (mean, stddev) = self.mean_stddev();
        let zscore = if stddev > 0.0 && self.is_ready() {
            (value - mean) / stddev
        } else {
            0.0
        };

        let label = if !self.is_ready() {
            "NORMAL"
        } else {
            match zscore {
                z if z <= 1.0 => "NORMAL",
                z if z <= 2.0 => "ELEVATED",
                z if z <= 3.0 => "HIGH",
                _ => "OUTLIER",
            }
        };

        self.push(value);

        Deviation {
            label,
            zscore,
            mean,
            stddev,
        }
    }

    fn push(&mut self, value: f64) {
        if self.window.len() >= self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(value);
    }

    fn mean_stddev(&self) -> (f64, f64) {
        let n = self.window.len() as f64;
        if n < 2.0 {
            return (self.window.front().copied().unwrap_or(0.0), 0.0);
        }
        let mean = self.window.iter().sum::<f64>() / n;
        let variance = self.window.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
        (mean, variance.sqrt())
    }
}
