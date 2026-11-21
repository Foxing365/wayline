//! Optional timer resolution for Windows AWDL scheduling experiments.
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};

const RESOLUTION_MS: u32 = 1;

pub(super) struct TimerResolution;

impl TimerResolution {
    pub(super) fn request() -> Option<Self> {
        // SAFETY: no pointers are passed. A successful request is balanced by Drop.
        let result = unsafe { timeBeginPeriod(RESOLUTION_MS) };
        if result != 0 {
            tracing::warn!(result, "Windows did not grant a 1 ms AWDL timer request");
            return None;
        }
        tracing::info!("requested 1 ms Windows timer resolution for RTL8821CU");
        Some(Self)
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        // SAFETY: request returned this guard only after timeBeginPeriod succeeded.
        unsafe { timeEndPeriod(RESOLUTION_MS) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::{sleep_until, Instant};

    #[test]
    #[ignore = "Windows scheduling microbenchmark, no radio or installed service needed"]
    fn sample_awdl_timer_lateness() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        // Alternate modes so changes in background load do not affect just one group.
        for high_resolution in [false, true, false, true, false, true] {
            let timer = high_resolution.then(TimerResolution::request).flatten();
            assert_eq!(timer.is_some(), high_resolution);
            let mut lateness = rt.block_on(async {
                let mut samples = Vec::with_capacity(100);
                for _ in 0..100 {
                    let target = Instant::now() + Duration::from_millis(4);
                    sleep_until(target).await;
                    samples.push(target.elapsed().as_micros());
                }
                samples
            });
            lateness.sort_unstable();
            println!(
                "timer_1ms={high_resolution} samples={} median_us={} p95_us={} max_us={}",
                lateness.len(),
                lateness[49],
                lateness[94],
                lateness[99]
            );
            drop(timer);
        }
    }
}
