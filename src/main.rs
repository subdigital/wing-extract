use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use std::cmp::min;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "wing_extract",
    version,
    about = "Extract interleaved multichannel WAV files into one mono WAV per channel."
)]
struct Args {
    /// Input WAV file(s) and/or directories containing WAV segment files.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Output directory for extracted channel WAV files.
    #[arg(short, long, default_value = "extracted")]
    output_dir: PathBuf,

    /// Prefix for output filenames. Files are written as <prefix>-<channel>.wav.
    #[arg(short, long, default_value = "Channel")]
    prefix: String,

    /// 1-based index for the first output channel filename suffix.
    #[arg(long, default_value_t = 1)]
    channel_start: usize,

    /// Stop after this many audio frames (for quick test extracts).
    #[arg(long)]
    max_frames: Option<u64>,

    /// Recursively scan input directories for WAV files.
    #[arg(long)]
    recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WavFormat {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    block_align: u16,
}

#[derive(Debug, Clone, Copy)]
struct DataRegion {
    offset: u64,
    len: u64,
}

#[derive(Debug, Clone)]
struct InputPart {
    path: PathBuf,
    format: WavFormat,
    data_regions: Vec<DataRegion>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

fn run(args: Args) -> Result<()> {
    let start = Instant::now();
    let input_files = resolve_inputs(&args.inputs, args.recursive)?;
    ensure!(!input_files.is_empty(), "no WAV files found");

    let mut parts = Vec::with_capacity(input_files.len());
    let mut expected_format = None::<WavFormat>;
    let mut total_frames = 0u64;

    for path in &input_files {
        let part = parse_input_part(path)?;
        ensure!(
            !part.data_regions.is_empty(),
            "no data chunk found in {}",
            path.display()
        );
        let data_bytes: u64 = part.data_regions.iter().map(|r| r.len).sum();
        ensure!(
            data_bytes % u64::from(part.format.block_align) == 0,
            "data length is not aligned to frame size in {}",
            path.display()
        );
        let frames = data_bytes / u64::from(part.format.block_align);
        total_frames = total_frames
            .checked_add(frames)
            .context("frame count overflow across inputs")?;

        if let Some(fmt) = expected_format {
            ensure!(
                fmt == part.format,
                "input formats differ; expected {:?}, got {:?} in {}",
                fmt,
                part.format,
                path.display()
            );
        } else {
            expected_format = Some(part.format);
        }

        parts.push(part);
    }

    let format = expected_format.context("missing WAV format")?;
    let frames_to_write = args
        .max_frames
        .map_or(total_frames, |n| n.min(total_frames));
    let duration_seconds = total_frames as f64 / f64::from(format.sample_rate);
    let (h, m, s) = hms(duration_seconds);
    println!(
        "Found {} WAV part(s), total input duration {:02}:{:02}:{:05.2}",
        parts.len(),
        h,
        m,
        s
    );
    ensure!(format.channels > 0, "WAV has zero channels");
    ensure!(
        format.audio_format == 1 || format.audio_format == 3,
        "unsupported WAV format {} (supported: PCM=1, IEEE float=3)",
        format.audio_format
    );
    let bytes_per_sample = bytes_per_sample(format.bits_per_sample);
    ensure!(
        bytes_per_sample > 0,
        "invalid bits per sample {}",
        format.bits_per_sample
    );
    ensure!(
        u16::from(format.channels) * bytes_per_sample as u16 == format.block_align,
        "invalid block align {} for {} channels @ {} bits",
        format.block_align,
        format.channels,
        format.bits_per_sample
    );

    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("failed to create {}", args.output_dir.display()))?;

    let channel_count = usize::from(format.channels);
    let mut writers = create_output_writers(
        &args.output_dir,
        &args.prefix,
        args.channel_start,
        channel_count,
        format,
        frames_to_write,
        bytes_per_sample,
    )?;

    demux_parts(
        &parts,
        &mut writers,
        format,
        bytes_per_sample,
        frames_to_write,
        start,
    )?;

    eprint!("\r{:<79}\r", "");
    let _ = std::io::stderr().flush();

    println!(
        "Extracted {} channels in {} ({} frames @ {} Hz, {}-bit) to {}",
        format.channels,
        format_duration(start.elapsed().as_secs()),
        frames_to_write,
        format.sample_rate,
        format.bits_per_sample,
        args.output_dir.display()
    );

    Ok(())
}

fn resolve_inputs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_file() {
            ensure!(is_wav_path(input), "not a WAV file: {}", input.display());
            files.push(input.clone());
            continue;
        }
        if input.is_dir() {
            collect_wavs(input, recursive, &mut files)?;
            continue;
        }
        bail!("input path does not exist: {}", input.display());
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_wavs(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read directory {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_file() && is_wav_path(&path) {
            out.push(path);
        } else if recursive && path.is_dir() {
            collect_wavs(&path, true, out)?;
        }
    }
    Ok(())
}

fn is_wav_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

fn parse_input_part(path: &Path) -> Result<InputPart> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let file_len = file
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    let mut r = BufReader::new(file);

    let mut riff = [0u8; 12];
    r.read_exact(&mut riff)
        .with_context(|| format!("failed to read WAV header {}", path.display()))?;
    ensure!(
        &riff[0..4] == b"RIFF",
        "not a RIFF file: {}",
        path.display()
    );
    ensure!(
        &riff[8..12] == b"WAVE",
        "not a WAVE file: {}",
        path.display()
    );

    let mut fmt = None::<WavFormat>;
    let mut data_regions = Vec::new();

    loop {
        let mut chunk_header = [0u8; 8];
        match r.read_exact(&mut chunk_header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("failed to read chunk header in {}", path.display()));
            }
        }
        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as u64;
        let payload_offset = r.stream_position()?;

        if chunk_id == b"fmt " {
            let mut fmt_buf = vec![0u8; chunk_size as usize];
            r.read_exact(&mut fmt_buf)
                .with_context(|| format!("failed to read fmt chunk in {}", path.display()))?;
            ensure!(
                fmt_buf.len() >= 16,
                "invalid fmt chunk length {} in {}",
                fmt_buf.len(),
                path.display()
            );
            let audio_format = u16::from_le_bytes([fmt_buf[0], fmt_buf[1]]);
            let channels = u16::from_le_bytes([fmt_buf[2], fmt_buf[3]]);
            let sample_rate = u32::from_le_bytes([fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7]]);
            let block_align = u16::from_le_bytes([fmt_buf[12], fmt_buf[13]]);
            let bits_per_sample = u16::from_le_bytes([fmt_buf[14], fmt_buf[15]]);

            fmt = Some(WavFormat {
                audio_format,
                channels,
                sample_rate,
                bits_per_sample,
                block_align,
            });
        } else if chunk_id == b"data" {
            let available = file_len.saturating_sub(payload_offset);
            let actual_len = min(chunk_size, available);
            data_regions.push(DataRegion {
                offset: payload_offset,
                len: actual_len,
            });
            r.seek(SeekFrom::Current(actual_len as i64))
                .with_context(|| format!("failed to seek data chunk in {}", path.display()))?;
            if chunk_size > actual_len {
                break;
            }
        } else {
            r.seek(SeekFrom::Current(chunk_size as i64))
                .with_context(|| format!("failed to skip chunk in {}", path.display()))?;
        }

        if chunk_size % 2 == 1 {
            r.seek(SeekFrom::Current(1))
                .with_context(|| format!("failed to skip chunk padding in {}", path.display()))?;
        }
    }

    let format = fmt.with_context(|| format!("missing fmt chunk in {}", path.display()))?;
    Ok(InputPart {
        path: path.to_path_buf(),
        format,
        data_regions,
    })
}

fn create_output_writers(
    output_dir: &Path,
    prefix: &str,
    channel_start: usize,
    channel_count: usize,
    format: WavFormat,
    total_frames: u64,
    bytes_per_sample: usize,
) -> Result<Vec<BufWriter<File>>> {
    let data_bytes_per_channel = total_frames
        .checked_mul(bytes_per_sample as u64)
        .context("output data size overflow")?;
    ensure!(
        data_bytes_per_channel <= u32::MAX as u64,
        "output channel exceeds WAV 4GB limit ({} bytes). RF64 output is not implemented.",
        data_bytes_per_channel
    );

    let mut writers = Vec::with_capacity(channel_count);
    for i in 0..channel_count {
        let channel_no = channel_start + i;
        let filename = format!("{prefix}-{channel_no}.wav");
        let path = output_dir.join(filename);
        let mut w = BufWriter::new(
            File::create(&path).with_context(|| format!("failed to create {}", path.display()))?,
        );
        write_wav_header(
            &mut w,
            format.audio_format,
            1,
            format.sample_rate,
            format.bits_per_sample,
            data_bytes_per_channel as u32,
        )?;
        writers.push(w);
    }
    Ok(writers)
}

fn demux_parts(
    parts: &[InputPart],
    writers: &mut [BufWriter<File>],
    format: WavFormat,
    bytes_per_sample: usize,
    mut frames_remaining: u64,
    start: Instant,
) -> Result<()> {
    let total_frames = frames_remaining;
    let channels = usize::from(format.channels);
    let frame_size = usize::from(format.block_align);
    let frames_per_chunk = 4096usize;
    let mut read_buf = vec![0u8; frames_per_chunk * frame_size];
    let mut channel_bufs = (0..channels)
        .map(|_| Vec::<u8>::with_capacity(frames_per_chunk * bytes_per_sample))
        .collect::<Vec<_>>();
    let mut frames_done = 0u64;

    for part in parts {
        let file = File::open(&part.path)
            .with_context(|| format!("failed to open {}", part.path.display()))?;
        let mut r = BufReader::new(file);
        for region in &part.data_regions {
            r.seek(SeekFrom::Start(region.offset))
                .with_context(|| format!("failed to seek data in {}", part.path.display()))?;
            let mut remaining = region.len;
            while remaining > 0 && frames_remaining > 0 {
                let mut to_read = min(read_buf.len() as u64, remaining) as usize;
                to_read -= to_read % frame_size;
                if to_read == 0 {
                    bail!(
                        "unaligned trailing bytes in {} data chunk",
                        part.path.display()
                    );
                }
                r.read_exact(&mut read_buf[..to_read]).with_context(|| {
                    format!("failed to read audio payload from {}", part.path.display())
                })?;

                let available_frames = to_read / frame_size;
                let allowed_frames = min(available_frames as u64, frames_remaining) as usize;
                let allowed_bytes = allowed_frames * frame_size;

                for b in &mut channel_bufs {
                    b.clear();
                }

                for frame in read_buf[..allowed_bytes].chunks_exact(frame_size) {
                    for ch in 0..channels {
                        let from = ch * bytes_per_sample;
                        let to = from + bytes_per_sample;
                        channel_bufs[ch].extend_from_slice(&frame[from..to]);
                    }
                }

                for (w, buf) in writers.iter_mut().zip(channel_bufs.iter()) {
                    w.write_all(buf)
                        .context("failed writing output channel data")?;
                }

                frames_done += allowed_frames as u64;
                frames_remaining -= allowed_frames as u64;
                remaining -= to_read as u64;
                print_progress(frames_done, total_frames, start.elapsed());
            }
            if frames_remaining == 0 {
                break;
            }
        }
        if frames_remaining == 0 {
            break;
        }
    }

    for w in writers {
        w.flush().context("failed to flush output file")?;
    }
    Ok(())
}

fn write_wav_header(
    w: &mut dyn Write,
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    data_size: u32,
) -> Result<()> {
    let bytes_per_sample = bytes_per_sample(bits_per_sample) as u16;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels) * u32::from(bytes_per_sample))
        .context("byte rate overflow")?;
    let block_align = channels * bytes_per_sample;
    let riff_size = 36u32.checked_add(data_size).context("RIFF size overflow")?;

    w.write_all(b"RIFF")?;
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&audio_format.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_size.to_le_bytes())?;
    Ok(())
}

fn bytes_per_sample(bits_per_sample: u16) -> usize {
    bits_per_sample.div_ceil(8) as usize
}

fn print_progress(done: u64, total: u64, elapsed: std::time::Duration) {
    static LAST_PRINT_MS: AtomicU64 = AtomicU64::new(0);
    let now_ms = elapsed.as_millis() as u64;
    let last_ms = LAST_PRINT_MS.load(Ordering::Relaxed);
    if done < total && now_ms.saturating_sub(last_ms) < 1000 {
        return;
    }
    LAST_PRINT_MS.store(now_ms, Ordering::Relaxed);

    let pct = if total > 0 { done as f64 / total as f64 } else { 0.0 };
    const BAR_WIDTH: usize = 30;
    let fill = ((pct * BAR_WIDTH as f64) as usize).min(BAR_WIDTH);
    let mut bar = vec![b' '; BAR_WIDTH];
    for b in bar[..fill].iter_mut() {
        *b = b'=';
    }
    if fill < BAR_WIDTH {
        bar[fill] = b'>';
    }
    let bar = std::str::from_utf8(&bar).unwrap();

    let elapsed_s = elapsed.as_secs();
    let eta = if done > 0 && done < total {
        let eta_s = (elapsed.as_secs_f64() * (total - done) as f64 / done as f64) as u64;
        format!(" | ETA {}", format_duration(eta_s))
    } else {
        String::new()
    };

    let line = format!(
        "[{bar}] {:3.0}% | {} elapsed{eta}",
        pct * 100.0,
        format_duration(elapsed_s),
    );
    eprint!("\r{line:<79}");
    let _ = std::io::stderr().flush();
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn hms(total_seconds: f64) -> (u64, u64, f64) {
    let h = (total_seconds / 3600.0).floor() as u64;
    let rem = total_seconds - (h as f64 * 3600.0);
    let m = (rem / 60.0).floor() as u64;
    let s = rem - (m as f64 * 60.0);
    (h, m, s)
}
