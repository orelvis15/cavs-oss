//! `cavs` — CLI for the CAVS-1 content-addressable video packaging format.
//!
//! Converts videos into `.cavs` (via ffmpeg CMAF/fMP4 segmentation),
//! reconstructs them back to playable MP4/HLS, inspects, verifies and plays.

mod classify;
mod corrupt;
mod doctor;
mod ffmpeg;
mod manifest_cmd;
mod pack;
mod profile;
mod report;
mod store;
mod sweep;
mod synth;
mod unpack;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cavs",
    version,
    about = "CAVS — content-addressable, deduplicated packaging",
    long_about = "CAVS packages files, game builds or video into .cavs: deduplicated \
                  FastCDC chunks, zstd-compressed and verifiable (BLAKE3 + Merkle + \
                  optional Ed25519 signature). Served by cavs-server, a client with a \
                  cache downloads only the bytes it doesn't already have.",
    after_help = "EXAMPLES:\n  \
        cavs pack --raw build_v42.pck -o v42.cavs           # a game release\n  \
        cavs pack --raw --sign-key pub.key data/* -o r.cavs # signed\n  \
        cavs pack movie.mp4 -o movie.cavs                   # video (segmented via ffmpeg)\n  \
        cavs info v42.cavs                                  # structure and dedupe\n  \
        cavs verify v42.cavs --pubkey pub.key.pub           # integrity + signature\n  \
        cavs unpack v42.cavs -o restored/                   # exact reconstruction\n\n\
        To serve and update clients: cavs-server / cavs-client --help"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ChunkModeArg {
    /// Fixed 256 KiB chunks (default for packaged media segments).
    Fixed,
    /// FastCDC 64/256/1024 KiB (default for raw assets).
    Cdc,
    /// Aggressive FastCDC 16/64/256 KiB (screen content, very repetitive data).
    Screen,
}

impl ChunkModeArg {
    pub fn to_mode(self, chunk_size: Option<usize>) -> cavs_chunker::ChunkMode {
        use cavs_chunker::ChunkMode;
        match self {
            ChunkModeArg::Fixed => ChunkMode::Fixed {
                size: chunk_size.unwrap_or(256 * 1024),
            },
            ChunkModeArg::Cdc => match chunk_size {
                Some(avg) => ChunkMode::Cdc {
                    min: (avg / 4).max(1024),
                    avg,
                    max: avg * 4,
                },
                None => ChunkMode::asset_default(),
            },
            ChunkModeArg::Screen => ChunkMode::screen_default(),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Package files (--raw) or videos into a deduplicated .cavs.
    ///
    /// With --raw it accepts any file (PCKs, bundles, binaries) and uses
    /// FastCDC 64 KiB + zstd 3 (the configuration validated in benchmarks).
    /// Without --raw it treats inputs as video: ffmpeg segments them into
    /// CMAF/fMP4 and CAVS packages the segments.
    Pack {
        /// Input video files (or arbitrary files with --raw).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Output .cavs path.
        #[arg(short, long)]
        output: PathBuf,
        /// Pack raw file bytes without ffmpeg segmentation (any file type).
        #[arg(long)]
        raw: bool,
        /// Target media segment duration in seconds (video mode).
        #[arg(long, default_value_t = 4.0)]
        segment_time: f64,
        /// Chunking strategy for media/asset payloads.
        #[arg(long, value_enum)]
        mode: Option<ChunkModeArg>,
        /// Chunk size in bytes (fixed size, or CDC average).
        #[arg(long)]
        chunk_size: Option<usize>,
        /// Chunk profile: `auto` classifies the payload and sweeps candidate
        /// profiles by cost, or force one of fixed-256k/fixed-512k/fixed-1m/
        /// fastcdc-64k/fastcdc-128k/fastcdc-256k. Overrides --mode.
        #[arg(long)]
        profile: Option<String>,
        /// Previous version of the (single) input, so `--profile auto`
        /// optimises for update egress instead of first install.
        #[arg(long)]
        prev: Option<PathBuf>,
        /// Also write a full bootstrap artifact (`<output>.bootstrap.zst`):
        /// the whole input zstd-compressed, so cache-less clients can install
        /// at full-artifact cost and seed their cache locally (raw mode,
        /// single input).
        #[arg(long)]
        bootstrap: bool,
        /// Disable zstd compression of stored chunks.
        #[arg(long)]
        no_compress: bool,
        /// zstd level for chunk storage/wire compression.
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
        /// Force re-encode (H.264/AAC) instead of trying stream copy first.
        #[arg(long)]
        transcode: bool,
        /// Sign the packed content with this Ed25519 secret key file
        /// (as produced by `cavs keygen`).
        #[arg(long)]
        sign_key: Option<PathBuf>,
    },
    /// Measure candidate chunk profiles on a payload (optionally against its
    /// previous version) and report the cheapest per cost model.
    Sweep {
        /// The payload to analyse (e.g. the new build).
        input: PathBuf,
        /// Previous version of the payload, to measure real chunk reuse.
        #[arg(long)]
        prev: Option<PathBuf>,
        /// Comma-separated profiles to test (default: recommended by the
        /// payload classifier).
        #[arg(long)]
        profiles: Option<String>,
        /// zstd level assumed for storage estimates.
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
        /// Write the full estimates as JSON to this path.
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Reconstruct the original media from a .cavs file.
    Unpack {
        input: PathBuf,
        /// Output directory.
        #[arg(short, long)]
        output: PathBuf,
        /// Skip writing the combined progressive .mp4 per video track.
        #[arg(long)]
        no_mp4: bool,
    },
    /// Show structure, dedup and compression statistics of a .cavs file.
    Info {
        input: PathBuf,
        /// Also list every segment.
        #[arg(long)]
        segments: bool,
        /// Also list every chunk.
        #[arg(long)]
        chunks: bool,
    },
    /// Verify every chunk hash, the Merkle root and all section hashes.
    Verify {
        input: PathBuf,
        /// Additionally require a valid content signature from this Ed25519
        /// public key (64 hex chars, or a path to a .pub file).
        #[arg(long)]
        pubkey: Option<String>,
    },
    /// Generate an Ed25519 signing keypair: <output> (secret, hex) and
    /// <output>.pub (public key, hex).
    Keygen {
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Reconstruct to a temp dir and play with ffplay.
    Play { input: PathBuf },
    /// Manage a global content-addressable store: ingest releases dedup'd
    /// across all versions/titles, unpublish, garbage collect.
    Store {
        /// Store directory (created if missing).
        dir: PathBuf,
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Inspect manifest formats: export readable JSON, benchmark
    /// json-v1 vs binary-v2 (v0.3.0 compact manifest).
    Manifest {
        #[command(subcommand)]
        action: ManifestAction,
    },
    /// Diagnose a deployment (v0.5.0): container integrity, manifest
    /// encodability, bootstrap sidecar, store/pack consistency, cache
    /// health. Read-only; exits non-zero on problems.
    Doctor {
        /// A .cavs file to check.
        input: Option<PathBuf>,
        /// A global store directory to check.
        #[arg(long)]
        store: Option<PathBuf>,
        /// A client chunk-cache directory to check.
        #[arg(long)]
        cache: Option<PathBuf>,
    },
    /// Hardening test harnesses (v0.5.0).
    Test {
        #[command(subcommand)]
        action: TestAction,
    },
    /// Synthetic large-build benchmarks (v0.5.0): generate deterministic
    /// datasets and measure the whole pipeline on them.
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
}

#[derive(Subcommand)]
enum TestAction {
    /// Corruption matrix: mutate a copy of the .cavs (and its manifest,
    /// packfile and bootstrap forms) byte by byte and assert every decoder
    /// rejects the corrupt artifact cleanly.
    Corrupt {
        /// The .cavs file to mutate (a pristine copy is used per test).
        input: PathBuf,
        /// Write the matrix report as JSON to this path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum BenchAction {
    /// Generate a deterministic synthetic dataset: a base build plus
    /// update variants (small/medium/large change, shifted, reordered).
    Gen {
        /// Output dataset directory.
        #[arg(long)]
        out: PathBuf,
        /// Base build size, e.g. 100MiB, 1GiB.
        #[arg(long, default_value = "100MiB")]
        size: String,
        /// PRNG seed (same seed + size => identical dataset).
        #[arg(long, default_value_t = 5)]
        seed: u64,
    },
    /// Pack and measure every version in a dataset directory: pack time,
    /// manifest sizes, dedup, update egress, packfile counts. Writes
    /// summary.md and summary.json.
    Suite {
        /// Dataset directory produced by `cavs bench gen`.
        #[arg(long)]
        dataset: PathBuf,
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum ManifestAction {
    /// Export the manifest of a .cavs as human-readable JSON (v1 format).
    Export {
        /// The .cavs file.
        input: PathBuf,
        /// Output path (stdout when omitted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Compare manifest formats for a .cavs: wire size, parse time,
    /// bytes per logical chunk.
    Bench {
        /// The .cavs file.
        input: PathBuf,
        /// Also write the report as JSON to this path.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum StorageArg {
    /// One file per chunk (pre-0.4.0 layout).
    Loose,
    /// Chunks appended into immutable .cavspack files, read by range.
    Packfiles,
}

#[derive(Subcommand)]
enum StoreAction {
    /// Ingest a .cavs into the store, deduplicating its chunks.
    Add {
        /// Asset name (e.g. game_v42).
        name: String,
        /// The .cavs file to ingest.
        cavs: PathBuf,
        /// Physical chunk layout; applies when the store is created (an
        /// existing store keeps its layout).
        #[arg(long, value_enum)]
        storage: Option<StorageArg>,
    },
    /// Unpublish an asset (chunks it uniquely held become reclaimable by gc).
    Rm { name: String },
    /// Remove zero-ref chunks that have been unreferenced for --grace
    /// seconds; a packfile is deleted once no live chunk references it.
    Gc {
        #[arg(long, default_value_t = 0)]
        grace: u64,
    },
    /// Show assets, storage savings and packfile occupancy.
    Stat,
    /// Re-hash every chunk (loose or packed) and check pack integrity.
    Verify,
    /// Export a packfile store as a deterministic immutable object tree,
    /// ready to upload to S3/R2/a static host behind a CDN.
    Export {
        /// Output directory (created if missing).
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Pack {
            inputs,
            output,
            raw,
            segment_time,
            mode,
            chunk_size,
            profile,
            prev,
            bootstrap,
            no_compress,
            zstd_level,
            transcode,
            sign_key,
        } => {
            let opts = pack::PackOptions {
                segment_time,
                mode,
                chunk_size,
                profile,
                prev,
                bootstrap,
                compress: !no_compress,
                zstd_level,
                force_transcode: transcode,
                sign_key,
            };
            if raw {
                pack::pack_raw(&inputs, &output, &opts)
            } else {
                pack::pack_video(&inputs, &output, &opts)
            }
        }
        Command::Sweep {
            input,
            prev,
            profiles,
            zstd_level,
            json,
        } => sweep::sweep(
            &input,
            prev.as_deref(),
            profiles.as_deref(),
            zstd_level,
            json.as_deref(),
        ),
        Command::Unpack {
            input,
            output,
            no_mp4,
        } => unpack::unpack(&input, &output, !no_mp4).map(|_| ()),
        Command::Info {
            input,
            segments,
            chunks,
        } => report::info(&input, segments, chunks),
        Command::Verify { input, pubkey } => report::verify(&input, pubkey.as_deref()),
        Command::Keygen { output } => keygen(&output),
        Command::Play { input } => unpack::play(&input),
        Command::Store { dir, action } => match action {
            StoreAction::Add {
                name,
                cavs,
                storage,
            } => store::add(&dir, &name, &cavs, storage),
            StoreAction::Rm { name } => store::remove(&dir, &name),
            StoreAction::Gc { grace } => store::gc(&dir, grace),
            StoreAction::Stat => store::stat(&dir),
            StoreAction::Verify => store::verify(&dir),
            StoreAction::Export { out } => store::export(&dir, &out),
        },
        Command::Manifest { action } => match action {
            ManifestAction::Export { input, out } => manifest_cmd::export(&input, out.as_deref()),
            ManifestAction::Bench { input, json } => manifest_cmd::bench(&input, json.as_deref()),
        },
        Command::Doctor {
            input,
            store,
            cache,
        } => doctor::doctor(input.as_deref(), store.as_deref(), cache.as_deref()),
        Command::Test { action } => match action {
            TestAction::Corrupt { input, out } => corrupt::corrupt(&input, out.as_deref()),
        },
        Command::Bench { action } => match action {
            BenchAction::Gen { out, size, seed } => synth::generate(&out, &size, seed),
            BenchAction::Suite { dataset, out } => synth::suite(&dataset, &out),
        },
    }
}

fn keygen(output: &std::path::Path) -> Result<()> {
    use rand_core::OsRng;
    let key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let secret_hex: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let public_hex: String = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(output, format!("{secret_hex}\n"))?;
    std::fs::write(output.with_extension("pub"), format!("{public_hex}\n"))?;
    println!("secret : {} (keep private)", output.display());
    println!("public : {}", output.with_extension("pub").display());
    println!("pubkey : {public_hex}");
    Ok(())
}
