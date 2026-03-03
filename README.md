# wing_extract

Extract interleaved multichannel WAV recordings (including split 4GB segments) into one mono WAV file per channel.

This project is specifically for extracting WAV files from a 32-channel packed WAV recording exported by Behringer WING consoles (including WING Rack/X-LIVE recordings).

## Why not LiveSessions?

I haven't had a single successful export using LiveSessions. Hence this utility.

## Download

1. Open this repository's **Releases** page.
2. Download the archive for your platform:
- macOS Apple Silicon: `aarch64-apple-darwin`
- macOS Intel: `x86_64-apple-darwin`
- Linux x64: `x86_64-unknown-linux-gnu`
- Windows x64: `x86_64-pc-windows-msvc`
3. Extract the archive.
4. Run the binary from a terminal:
- macOS/Linux: `./wing_extract ...`
- Windows (PowerShell): `.\\wing_extract.exe ...`

## Usage

```bash
wing_extract /Volumes/WING_SD/X_LIVE/5C5C992D --output-dir ./out
```

Pass either:
- a directory containing WAV segments (for example `00000001.WAV`, `00000002.WAV`, ...), or
- one or more WAV files/directories explicitly.

```bash
# Directory mode
wing_extract /Volumes/WING_SD/X_LIVE/5C5C992D --output-dir ./out

# Explicit file mode
wing_extract 00000001.WAV 00000002.WAV 00000003.WAV --output-dir ./out

# Recursive mode (scan subfolders for long recordings split across folders)
wing_extract /Volumes/WING_SD/X_LIVE --recursive --output-dir ./out
```

Useful options:

```text
--prefix <name>         Output file prefix (default: Channel)
--channel-start <n>     First output channel index (default: 1)
--max-frames <n>        Limit extraction for quick checks
--recursive             Recursively scan input directories for WAV files
```

Output files are named like `Channel-1.wav`, `Channel-2.wav`, etc.

## Build From Source (Optional)

```bash
cargo build --release
```

## License

MIT. See [LICENSE](LICENSE).
