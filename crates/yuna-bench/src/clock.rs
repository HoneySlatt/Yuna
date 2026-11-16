//! The clock the bench marks its windows on: CLOCK_MONOTONIC, the one KWin stamps its frames
//! with and its frame counter prints. The standard library's `Instant` reads it too, but gives no
//! way to compare it with another program's.

/// Now, on CLOCK_MONOTONIC, in nanoseconds.
pub fn monotonic_ns() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    // The monotonic clock never goes below zero.
    now.tv_sec.max(0) as u64 * 1_000_000_000 + now.tv_nsec.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_moves_on() {
        let before = monotonic_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(monotonic_ns() >= before + 2_000_000);
    }
}
