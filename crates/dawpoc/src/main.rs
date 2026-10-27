//! `dawpoc`: proof of concept for the DAW engine. Hosts VST3 effects, renders offline through
//! the shared processing path, null-tests renders and benchmarks multithreaded processing.

mod bench;
mod engine;
mod null;
mod signal;

use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use daw_core::{plugins, wav};
use engine::Chain;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vst3_host::ProcessMode;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Realtime,
    Offline,
}

impl From<Mode> for ProcessMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Realtime => ProcessMode::Realtime,
            Mode::Offline => ProcessMode::Offline,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// List installed .vst3 bundles without loading them.
    List {
        /// Case-insensitive substring filter.
        filter: Option<String>,
    },
    /// Write the deterministic stereo test signal as a 32-bit float WAV (or, with --guitar, a
    /// mono plucked-guitar riff: the demo's audio where no real recording may be used).
    Gen {
        #[arg(long, default_value = "test.wav")]
        output: PathBuf,
        /// A plucked guitar riff, mono, instead of the test signal.
        #[arg(long)]
        guitar: bool,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        #[arg(long, default_value_t = 48_000)]
        sample_rate: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Load a plugin and print its buses, latency, tail and parameters.
    Info {
        #[arg(long)]
        plugin: String,
        #[arg(long, value_enum, default_value = "realtime")]
        mode: Mode,
        #[arg(long, default_value_t = 512)]
        block: usize,
        #[arg(long, default_value_t = 48_000.0)]
        sample_rate: f64,
        /// Print every parameter rather than the first 30.
        #[arg(long)]
        all_params: bool,
    },
    /// Render a WAV through a chain of plugins.
    Render {
        /// Plugin path or installed name; repeat to build a chain (processed in order).
        #[arg(long, required = true)]
        plugin: Vec<String>,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "realtime")]
        mode: Mode,
        #[arg(long, default_value_t = 512)]
        block: usize,
        /// Preset for the plugin at the same position (.vstpreset, or vst3-host JSON).
        #[arg(long)]
        preset: Vec<PathBuf>,
        /// NAME=VALUE (normalized 0..1), applied to every plugin that has the parameter.
        #[arg(long = "set", value_parser = plugins::parse_param)]
        params: Vec<(String, f64)>,
        /// Append the reported tail (capped by --max-tail) instead of matching input length.
        #[arg(long)]
        keep_tail: bool,
        #[arg(long, default_value_t = 10.0)]
        max_tail: f64,
        /// Drop plugins that fail to load instead of aborting.
        #[arg(long)]
        skip_failed: bool,
        /// Wait this long (wall clock) after activation before processing, for plugins that
        /// finish initializing on a background thread.
        #[arg(long, default_value_t = 0)]
        settle_ms: u64,
        /// Process this many seconds of silence, discarded, before the render starts.
        #[arg(long, default_value_t = 0.0)]
        preroll: f64,
    },
    /// Null two WAV files: subtract them and report the residual.
    Null {
        #[arg(long)]
        a: PathBuf,
        #[arg(long)]
        b: PathBuf,
        /// Also search lags of +-N samples by cross-correlation and null at the best one.
        #[arg(long)]
        search_lag: Option<usize>,
        /// Print the residual per segment of this many seconds, to see where renders diverge.
        #[arg(long)]
        segment: Option<f64>,
    },
    /// Process audio through N instances serially, then in parallel, and compare.
    Bench {
        #[arg(long)]
        plugin: String,
        #[arg(long, default_value_t = 4)]
        instances: usize,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        /// Worker threads for the parallel run (default: one per instance).
        #[arg(long)]
        threads: Option<usize>,
        #[arg(long, default_value_t = 512)]
        block: usize,
        #[arg(long, value_enum, default_value = "realtime")]
        mode: Mode,
        #[arg(long, default_value_t = 48_000.0)]
        sample_rate: f64,
        /// NAME=VALUE (normalized 0..1), applied to every instance.
        #[arg(long = "set", value_parser = plugins::parse_param)]
        params: Vec<(String, f64)>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::List { filter } => list(filter.as_deref()),
        Command::Gen {
            output,
            guitar,
            seconds,
            sample_rate,
            seed,
        } => gen(&output, guitar, seconds, sample_rate, seed),
        Command::Info {
            plugin,
            mode,
            block,
            sample_rate,
            all_params,
        } => info(&plugin, mode.into(), block, sample_rate, all_params),
        Command::Render {
            plugin,
            input,
            output,
            mode,
            block,
            preset,
            params,
            keep_tail,
            max_tail,
            skip_failed,
            settle_ms,
            preroll,
        } => render(RenderArgs {
            plugins: &plugin,
            input: &input,
            output: &output,
            mode: mode.into(),
            block,
            presets: &preset,
            params: &params,
            keep_tail,
            max_tail,
            skip_failed,
            settle_ms,
            preroll,
        }),
        Command::Null {
            a,
            b,
            search_lag,
            segment,
        } => null_cmd(&a, &b, search_lag, segment),
        Command::Bench {
            plugin,
            instances,
            seconds,
            threads,
            block,
            mode,
            sample_rate,
            params,
        } => bench_cmd(BenchArgs {
            spec: &plugin,
            instances,
            seconds,
            threads: threads.unwrap_or(instances),
            block,
            mode: mode.into(),
            sample_rate,
            params: &params,
        }),
    }
}

fn list(filter: Option<&str>) -> Result<()> {
    let host = plugins::host(48_000.0, 512)?;
    let needle = filter.map(str::to_lowercase);
    for path in plugins::installed(&host) {
        let shown = path.display().to_string();
        if needle
            .as_ref()
            .is_none_or(|n| shown.to_lowercase().contains(n))
        {
            println!("{shown}");
        }
    }
    Ok(())
}

fn gen(output: &Path, guitar: bool, seconds: f64, sample_rate: u32, seed: u64) -> Result<()> {
    ensure!(seconds > 0.0, "--seconds must be positive");
    let channels = if guitar {
        vec![signal::guitar(sample_rate, seconds, seed)]
    } else {
        signal::generate(sample_rate, seconds, seed)
    };
    let audio = wav::Audio {
        sample_rate,
        channels,
    };
    wav::write_f32(output, &audio)?;
    let s = stats(&audio.channels);
    println!(
        "wrote {} ({:.2} s, {} Hz, {} ch, f32, seed {seed}), peak {:.2} dBFS, rms {:.2} dBFS",
        output.display(),
        audio.seconds(),
        sample_rate,
        audio.channels.len(),
        null::dbfs(s.peak),
        null::dbfs(s.rms)
    );
    Ok(())
}

fn info(spec: &str, mode: ProcessMode, block: usize, sample_rate: f64, all: bool) -> Result<()> {
    let mut host = plugins::host(sample_rate, block)?;
    let path = plugins::resolve(spec, &plugins::installed(&host))?;
    let name = plugins::display_name(&path);
    let plugin = plugins::load(&mut host, &path, None)?;
    let i = plugin.info();
    println!(
        "{} by {} (v{}, {})",
        i.name, i.vendor, i.version, i.category
    );
    println!("path: {}", path.display());
    println!(
        "latency at load (vst3-host runs setupProcessing at its 44.1 kHz default here): {}",
        plugin.latency_samples()
    );
    let params = plugin.get_parameters().unwrap_or_default();
    let shown = if all {
        params.len()
    } else {
        params.len().min(30)
    };
    let formatted: Vec<String> = params[..shown]
        .iter()
        .map(|p| plugin.format_parameter(p.id, p.value).unwrap_or_default())
        .collect();
    let mut chain = Chain::new(2, block, mode);
    chain.push(&name, plugin)?;
    println!("{:?} mode:", chain.mode());
    print_stage(&chain.stages()[0]);
    // Switching modes on a live chain is what preview/bounce toggling will do.
    let other = match mode {
        ProcessMode::Offline => ProcessMode::Realtime,
        _ => ProcessMode::Offline,
    };
    chain.set_process_mode(other)?;
    println!("{:?} mode:", chain.mode());
    print_stage(&chain.stages()[0]);
    println!("parameters: {}", params.len());
    for (p, text) in params.iter().zip(&formatted) {
        println!(
            "  [{}] {:?} = {:.4} ({}{})",
            p.id,
            p.name,
            p.value,
            text,
            unit(&p.unit)
        );
    }
    if shown < params.len() {
        println!("  ... {} more (use --all-params)", params.len() - shown);
    }
    Ok(())
}

fn unit(u: &str) -> String {
    if u.is_empty() {
        String::new()
    } else {
        format!(" {}", u.trim())
    }
}

fn print_stage(stage: &engine::Stage) {
    let buses = |b: &[vst3_host::AudioBusConfig]| {
        b.iter()
            .map(|c| {
                format!(
                    "{}ch{}",
                    c.channel_count,
                    if c.active { "" } else { " (off)" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let tail = match stage.tail() {
        u32::MAX => "infinite".to_string(),
        t => t.to_string(),
    };
    let flags = stage.restart_flags();
    let restart = if flags.is_empty() {
        String::new()
    } else {
        format!(
            " (restart requested on activation: flags {:#x}, latency changed: {})",
            flags.bits(),
            flags.latency_changed()
        )
    };
    println!(
        "  {}: inputs [{}] outputs [{}] latency {} tail {}{restart}",
        stage.name(),
        buses(&stage.layout().inputs),
        buses(&stage.layout().outputs),
        stage.latency(),
        tail
    );
}

struct RenderArgs<'a> {
    plugins: &'a [String],
    input: &'a Path,
    output: &'a Path,
    mode: ProcessMode,
    block: usize,
    presets: &'a [PathBuf],
    params: &'a [(String, f64)],
    keep_tail: bool,
    max_tail: f64,
    skip_failed: bool,
    settle_ms: u64,
    preroll: f64,
}

fn render(args: RenderArgs) -> Result<()> {
    ensure!(args.block > 0, "--block must be positive");
    ensure!(
        args.presets.len() <= args.plugins.len(),
        "more --preset than --plugin arguments"
    );
    let input = wav::read(args.input)?;
    let sample_rate = f64::from(input.sample_rate);
    let mut host = plugins::host(sample_rate, args.block)?;
    let installed = plugins::installed(&host);

    let mut chain = Chain::new(input.num_channels(), args.block, args.mode);
    let mut applied = vec![false; args.params.len()];
    for (index, spec) in args.plugins.iter().enumerate() {
        let added = (|| -> Result<()> {
            let path = plugins::resolve(spec, &installed)?;
            let name = plugins::display_name(&path);
            let t = Instant::now();
            let mut plugin = plugins::load(
                &mut host,
                &path,
                args.presets.get(index).map(PathBuf::as_path),
            )?;
            let names: Vec<String> = plugin
                .get_parameters()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.name)
                .collect();
            for (k, (param, value)) in args.params.iter().enumerate() {
                if names.contains(param) {
                    plugin
                        .set_parameter_by_name(param, *value)
                        .with_context(|| format!("{name}: setting '{param}'"))?;
                    applied[k] = true;
                }
            }
            chain.push(&name, plugin)?;
            eprintln!("loaded {name} in {:.2} s", t.elapsed().as_secs_f64());
            Ok(())
        })();
        if let Err(e) = added {
            if args.skip_failed {
                eprintln!("SKIPPED plugin '{spec}': {e:#}");
            } else {
                return Err(e);
            }
        }
    }
    if let Some(k) = applied.iter().position(|a| !a) {
        bail!(
            "no plugin in the chain has a parameter named '{}'",
            args.params[k].0
        );
    }
    ensure!(!chain.stages().is_empty(), "no plugin could be loaded");

    println!(
        "mode {:?}, block {}, {} Hz",
        chain.mode(),
        args.block,
        sample_rate
    );
    for stage in chain.stages() {
        print_stage(stage);
    }
    let latency = chain.latency();
    let max_tail = (args.max_tail * sample_rate) as usize;
    let tail = chain.tail().unwrap_or(usize::MAX).min(max_tail);
    let out_len = input.len() + if args.keep_tail { tail } else { 0 };
    println!(
        "chain latency {latency} samples ({:.2} ms), tail {}",
        latency as f64 * 1000.0 / sample_rate,
        match chain.tail() {
            Some(t) => format!("{t} samples"),
            None => "infinite".into(),
        }
    );

    if args.settle_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(args.settle_ms));
    }
    if args.preroll > 0.0 {
        let blocks = bench::blocks_for(args.preroll, sample_rate, args.block);
        let mut silence = vec![vec![0.0; args.block]; chain.channels()];
        for _ in 0..blocks {
            silence.iter_mut().for_each(|c| c.fill(0.0));
            chain.process_block(&mut silence)?;
        }
        println!("pre-rolled {} blocks of silence", blocks);
    }

    let t = Instant::now();
    let out = engine::render_aligned(&input.channels, args.block, latency, out_len, |io| {
        chain.process_block(io)
    })?;
    let wall = t.elapsed().as_secs_f64();
    let audio_secs = (out_len + latency) as f64 / sample_rate;
    println!(
        "rendered {:.2} s of audio in {:.3} s wall ({:.1}x realtime)",
        audio_secs,
        wall,
        audio_secs / wall
    );

    let s = stats(&out);
    let diff = null::null(&input.channels, &out, 0);
    println!(
        "output: peak {:.2} dBFS, rms {:.2} dBFS, non-finite samples {}; vs input: residual peak {:.2} dBFS, rms {:.2} dBFS",
        null::dbfs(s.peak),
        null::dbfs(s.rms),
        s.non_finite,
        null::dbfs(diff.peak),
        null::dbfs(diff.rms)
    );
    if s.non_finite > 0 {
        eprintln!("WARNING: output contains NaN or infinity");
    }
    if s.peak == 0.0 {
        eprintln!("WARNING: output is completely silent");
    }
    if diff.differing == 0 {
        eprintln!("WARNING: output is identical to the input (plugin bypassed?)");
    }

    wav::write_f32(
        args.output,
        &wav::Audio {
            sample_rate: input.sample_rate,
            channels: out,
        },
    )?;
    println!("wrote {}", args.output.display());
    Ok(())
}

struct Stats {
    peak: f64,
    rms: f64,
    non_finite: usize,
}

fn stats(channels: &[Vec<f32>]) -> Stats {
    let mut peak = 0.0f64;
    let mut sq = 0.0f64;
    let mut count = 0usize;
    let mut non_finite = 0;
    for &x in channels.iter().flatten() {
        if !x.is_finite() {
            non_finite += 1;
            continue;
        }
        let x = f64::from(x);
        peak = peak.max(x.abs());
        sq += x * x;
        count += 1;
    }
    Stats {
        peak,
        rms: (sq / count.max(1) as f64).sqrt(),
        non_finite,
    }
}

fn null_cmd(a: &Path, b: &Path, search_lag: Option<usize>, segment: Option<f64>) -> Result<()> {
    let wa = wav::read(a)?;
    let wb = wav::read(b)?;
    if wa.sample_rate != wb.sample_rate {
        eprintln!(
            "WARNING: sample rates differ ({} vs {})",
            wa.sample_rate, wb.sample_rate
        );
    }
    let report = null::null(&wa.channels, &wb.channels, 0);
    print_null("aligned (lag 0)", &report);
    if let Some(seconds) = segment {
        let len = (seconds * f64::from(wa.sample_rate)).round().max(1.0) as usize;
        println!("  per-segment residual (time: peak / rms dBFS):");
        for (start, peak, rms) in null::segments(&wa.channels, &wb.channels, 0, len) {
            println!(
                "    {:>7.2} s: {:>8.2} / {:>8.2}",
                start as f64 / f64::from(wa.sample_rate),
                null::dbfs(peak),
                null::dbfs(rms)
            );
        }
    }

    if let Some(max_lag) = search_lag {
        let t = Instant::now();
        let (lag, r) = null::best_lag(
            &null::mono(&wa.channels),
            &null::mono(&wb.channels),
            max_lag,
            1 << 17,
        );
        println!(
            "lag search +-{max_lag}: best lag {lag} samples (b is {} a), correlation {r:.6} ({:.2} s)",
            match lag.signum() {
                1 => "behind",
                -1 => "ahead of",
                _ => "aligned with",
            },
            t.elapsed().as_secs_f64()
        );
        if lag != 0 {
            print_null(
                &format!("at lag {lag}"),
                &null::null(&wa.channels, &wb.channels, lag),
            );
        }
    }
    Ok(())
}

fn print_null(label: &str, r: &null::NullReport) {
    println!("{label}:");
    if r.len_a != r.len_b || r.channels_a != r.channels_b {
        println!(
            "  shapes differ: a {} ch x {}, b {} ch x {}; compared the overlap",
            r.channels_a, r.len_a, r.channels_b, r.len_b
        );
    }
    println!(
        "  residual peak {:.2} dBFS, rms {:.2} dBFS (reference peak {:.2} dBFS, rms {:.2} dBFS)",
        null::dbfs(r.peak),
        null::dbfs(r.rms),
        null::dbfs(r.ref_peak),
        null::dbfs(r.ref_rms)
    );
    let total = r.compared * r.channels_a.min(r.channels_b);
    println!(
        "  {} of {} samples differ ({:.3}%), non-finite {}",
        r.differing,
        total,
        100.0 * r.differing as f64 / total.max(1) as f64,
        r.non_finite
    );
    println!("  verdict: {}", r.verdict());
}

struct BenchArgs<'a> {
    spec: &'a str,
    instances: usize,
    seconds: f64,
    threads: usize,
    block: usize,
    mode: ProcessMode,
    sample_rate: f64,
    params: &'a [(String, f64)],
}

fn bench_cmd(args: BenchArgs) -> Result<()> {
    let BenchArgs {
        spec,
        instances,
        seconds,
        threads,
        block,
        mode,
        sample_rate,
        params,
    } = args;
    ensure!(
        instances > 0 && block > 0 && seconds > 0.0,
        "invalid arguments"
    );
    let mut host = plugins::host(sample_rate, block)?;
    let path = plugins::resolve(spec, &plugins::installed(&host))?;
    let name = plugins::display_name(&path);
    let signal = signal::generate(sample_rate as u32, 10.0, 1);

    let t = Instant::now();
    let mut pool = Vec::with_capacity(instances);
    for i in 0..instances {
        let mut plugin = plugins::load(&mut host, &path, None)
            .with_context(|| format!("loading instance {i}"))?;
        for (param, value) in params {
            plugin
                .set_parameter_by_name(param, *value)
                .with_context(|| format!("{name}: setting '{param}'"))?;
        }
        let mut chain = Chain::new(2, block, mode);
        chain.push(format!("{name} #{i}"), plugin)?;
        pool.push(bench::Instance::new(chain));
    }
    println!(
        "{name}: loaded {instances} instances in {:.2} s (mode {mode:?}, block {block}, latency {})",
        t.elapsed().as_secs_f64(),
        pool[0].chain.latency()
    );

    let r = bench::run(&mut pool, &signal, sample_rate, seconds, threads)?;
    let total = r.audio_seconds_total();
    println!(
        "{} instances x {:.2} s audio ({} blocks of {block}), {} threads, block deadline {:.2} ms",
        r.instances,
        r.audio_seconds_per_instance,
        r.blocks,
        r.threads,
        r.block_deadline.as_secs_f64() * 1e3
    );
    for (label, t) in [("serial", &r.serial), ("parallel", &r.parallel)] {
        let rt = bench::BenchReport::realtime_factor(t, total);
        println!(
            "  {label:<8} wall {:>8.3} s  aggregate {:>7.1}x realtime  per-instance {:>7.1}x  worst block {:.3} ms",
            t.wall.as_secs_f64(),
            rt,
            rt / r.instances as f64,
            t.worst_block.as_secs_f64() * 1e3
        );
    }
    println!(
        "  speedup {:.2}x on {} threads ({:.0}% parallel efficiency)",
        r.speedup(),
        r.threads,
        100.0 * r.speedup() / r.threads as f64
    );
    Ok(())
}
