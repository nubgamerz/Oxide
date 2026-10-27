//! CPU topology and real-time thread setup.

/// Number of physical cores (SMT siblings counted once).
pub fn physical_cores() -> usize {
    #[cfg(windows)]
    {
        windows::physical_cores()
    }
    #[cfg(not(windows))]
    {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
}

/// Default number of audio worker threads: one per physical core, minus one for the audio
/// driver's own thread (which also does work) and the rest of the system.
pub fn default_workers() -> usize {
    physical_cores().saturating_sub(1)
}

/// Give the current thread audio priority.
///
/// Threads are deliberately not pinned to cores. Pinning was measured to cause rare stalls
/// of several buffer periods at small buffer sizes: the driver's audio thread, which is not
/// ours to pin, would land on a worker's core and the two would fight over it.
pub fn make_realtime() {
    #[cfg(windows)]
    windows::make_realtime(MMCSS.load(std::sync::atomic::Ordering::Relaxed));
}

static MMCSS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether threads made real-time from now on register with MMCSS ("Pro Audio", the default)
/// or only take `THREAD_PRIORITY_TIME_CRITICAL`. For measurements (`live_bench`, TD9).
pub fn use_mmcss(on: bool) {
    MMCSS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The whole process's CPU time so far (user and kernel, every thread: the audio thread, the
/// workers, the UI and the plug-ins' own threads, those that have exited too), in seconds; 0
/// where it can't be read.
///
/// On x86-64 Windows it counts cycles (`QueryProcessCycleTime`, which Windows keeps from the
/// time-stamp counter at every context switch), not `GetProcessTimes`: that one is sampled at
/// the 15.6 ms clock tick, charging each tick whole to the thread on the CPU at that instant,
/// and a busy machine's threads, or a DAW's short bursts per callback, are off the CPU at most
/// ticks. Measured under a parallel test run, an 80 ms spin read 0 s there and 0.83-0.94 of a
/// core in cycles (TD156).
pub fn process_time() -> f64 {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        windows::process_cycles() as f64 / windows::counter_rate()
    }
    #[cfg(all(windows, not(target_arch = "x86_64")))]
    {
        windows::process_time()
    }
    #[cfg(not(windows))]
    {
        0.0
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority,
        THREAD_PRIORITY_TIME_CRITICAL,
    };

    pub fn physical_cores() -> usize {
        let mut len = 0u32;
        // SAFETY: a null buffer asks for the required length.
        unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len) };
        let entry = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        let mut info =
            vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); len as usize / entry + 1];
        let mut bytes = (info.len() * entry) as u32;
        // SAFETY: the buffer holds `bytes` bytes of properly aligned entries.
        if unsafe { GetLogicalProcessorInformation(info.as_mut_ptr(), &mut bytes) } == 0 {
            return std::thread::available_parallelism().map_or(1, |n| n.get());
        }
        info.truncate(bytes as usize / entry);
        info.iter()
            .filter(|i| i.Relationship == RelationProcessorCore)
            .count()
            .max(1)
    }

    /// The process's cycles so far, every thread's (0 where they can't be read). They count at
    /// the time-stamp counter's fixed rate, whatever the cores' clocks do.
    #[cfg(target_arch = "x86_64")]
    pub fn process_cycles() -> u64 {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;
        let mut cycles = 0u64;
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing, and `cycles`
        // is ours to fill for the length of the call.
        if unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut cycles) } == 0 {
            return 0;
        }
        cycles
    }

    /// The time-stamp counter's ticks per second, measured once against `Instant` over 2 ms
    /// (the first call spins that long) and then fixed: a rate that kept changing would move
    /// every reading by the whole process's time so far times the change. The counter runs
    /// whether this thread does or not, so a busy machine can't skew it, as it would a rate
    /// taken from a thread's own cycles while it spins.
    #[cfg(target_arch = "x86_64")]
    pub fn counter_rate() -> f64 {
        use std::sync::OnceLock;
        use std::time::Duration;
        static RATE: OnceLock<f64> = OnceLock::new();
        *RATE.get_or_init(|| {
            let (start, ticks) = counter_at();
            while start.elapsed() < Duration::from_millis(2) {
                std::hint::spin_loop();
            }
            let (end, end_ticks) = counter_at();
            end_ticks.saturating_sub(ticks) as f64 / (end - start).as_secs_f64()
        })
    }

    /// An `Instant` and the counter read together: between two counter reads close enough
    /// that nothing took the thread off the CPU in between (a few tries), so the pair is
    /// within a microsecond of itself.
    #[cfg(target_arch = "x86_64")]
    fn counter_at() -> (std::time::Instant, u64) {
        let mut best = None;
        for _ in 0..100 {
            let before = tsc();
            let at = std::time::Instant::now();
            let after = tsc();
            let gap = after.saturating_sub(before);
            let pair = (at, before + gap / 2);
            if gap < 5_000 {
                return pair;
            }
            if best.is_none_or(|(_, g)| gap < g) {
                best = Some((pair, gap));
            }
        }
        best.expect("a hundred tries").0
    }

    #[cfg(target_arch = "x86_64")]
    fn tsc() -> u64 {
        // SAFETY: RDTSC exists on every x86-64 CPU and only reads the counter.
        unsafe { std::arch::x86_64::_rdtsc() }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn process_time() -> f64 {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing, and the four
        // FILETIMEs are ours to fill for the length of the call.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return 0.0;
        }
        let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
        // FILETIME counts 100 ns.
        (ticks(kernel) + ticks(user)) as f64 * 1e-7
    }

    pub fn make_realtime(mmcss: bool) {
        let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let mut index = 0u32;
        // SAFETY: plain Win32 calls on the current thread's pseudo-handle.
        unsafe {
            // MMCSS boosts the thread like the audio driver's own; fall back to a high priority.
            if !mmcss || AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index).is_null() {
                SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_cores() {
        let cores = physical_cores();
        let logical = std::thread::available_parallelism().unwrap().get();
        assert!(cores >= 1 && cores <= logical);
    }

    /// Work the process did is counted whether or not a clock tick caught it on the CPU
    /// (TD156): a fixed amount of work, far shorter than a tick, always moves the process's
    /// time, by at least this thread's own share of it.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn process_time_counts_short_work() {
        for _ in 0..20 {
            let before = process_time();
            let thread = crate::trace::thread_cycles();
            let mut x = 0u64;
            for _ in 0..20_000 {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
            let own = (crate::trace::thread_cycles() - thread) as f64 / windows::counter_rate();
            let counted = process_time() - before;
            assert!(own > 0.0, "the thread's cycles moved");
            assert!(
                counted >= own * 0.999,
                "{counted} s counted, {own} s on this thread"
            );
        }
    }
}
