# wing_extract

Extract interleaved multichannel WAV recordings (including split 4GB segments) into one mono WAV file per channel.

This project is specifically for extracting WAV files from a 32-channel packed WAV recording exported by Behringer WING consoles (including WING Rack/X-LIVE recordings).

## Why not LiveSessions?

I haven't had a single successful export using LiveSessions. Hence this utility.

## Build

```bash
cargo build --release
```

## Usage

Pass either:
- a directory containing WAV segments (for example `00000001.WAV`, `00000002.WAV`, ...), or
- one or more WAV files/directories explicitly.

```bash
# Directory mode
cargo run --release -- /Volumes/WING_SD/X_LIVE/5C5C992D --output-dir ./out

# Explicit file mode
cargo run --release -- 00000001.WAV 00000002.WAV 00000003.WAV --output-dir ./out

# Recursive mode (scan subfolders for long recordings split across folders)
cargo run --release -- /Volumes/WING_SD/X_LIVE --recursive --output-dir ./out
```

Useful options:

```text
--prefix <name>         Output file prefix (default: Channel)
--channel-start <n>     First output channel index (default: 1)
--max-frames <n>        Limit extraction for quick checks
--recursive             Recursively scan input directories for WAV files
```

Output files are named like `Channel-1.wav`, `Channel-2.wav`, etc.

## License

MIT. See [LICENSE](LICENSE).
