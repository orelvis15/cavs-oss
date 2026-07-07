//! `cavs` — CLI for the CAVS-1 content-addressable video packaging format.
//!
//! Converts videos into `.cavs` (via ffmpeg CMAF/fMP4 segmentation),
//! reconstructs them back to playable MP4/HLS, inspects, verifies and plays.

mod apply_cmd;
mod bench_butler;
mod bench_butler_full;
mod bench_compression;
mod bench_delta;
mod bench_pairwise;
mod bench_pipeline;
mod bench_routes;
mod bench_versions;
mod blob_detect;
mod classify;
mod compare;
mod corrupt;
mod diff_plan;
mod doctor;
mod ffmpeg;
mod ignore;
mod inspect_cmd;
mod manifest_cmd;
mod optimize_patch;
mod pack;
mod pack_dir;
mod patch_policy;
mod patch_v2;
mod preview;
mod profile;
mod publish_dir;
mod report;
mod route_plan;
mod signature_cmd;
mod store;
mod sweep;
mod synth;
mod test_recovery;
mod tool_metrics;
mod unpack;
mod verify_install;

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
        /// Report reusable bytes against a previous version's `.cavssig`
        /// (v0.6.0 hybrid reconstruction; raw mode).
        #[arg(long)]
        against_signature: Option<PathBuf>,
    },
    /// Package a directory tree as a container asset (v0.6.0 preview):
    /// one deduplicated data track per file, plus directory/symlink/exec
    /// metadata. Clients apply it with per-file no-op detection and staged
    /// installs.
    PackDir {
        /// The directory to package.
        input: PathBuf,
        /// Output .cavs path.
        #[arg(short, long)]
        output: PathBuf,
        /// Chunk profile label (fixed-256k/…/fastcdc-64k…); default (and
        /// `auto`) is the update-validated fastcdc-64k.
        #[arg(long)]
        profile: Option<String>,
        /// Disable zstd compression of stored chunks.
        #[arg(long)]
        no_compress: bool,
        /// zstd level for chunk storage/wire compression.
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
        /// Sign the packed content with this Ed25519 secret key file.
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// Exclude entries matching this glob (repeatable; merged with the
        /// tree root's `.cavsignore`). `*`/`?` stay in one segment, `**`
        /// crosses, a trailing `/` ignores a whole directory.
        #[arg(long)]
        ignore: Vec<String>,
    },
    /// Export, inspect, list and verify compact `.cavssig` signatures:
    /// the old version's layout and block hashes, so new versions can be
    /// planned against it without the old bytes.
    Signature {
        #[command(subcommand)]
        action: SignatureAction,
    },
    /// Compare a new build against a previous version's `.cavssig`:
    /// NEW/MODIFIED/DELETED/SAME per entry, estimated update sizes per
    /// route, and warnings for patch-hostile (compressed) files.
    Preview {
        /// The new build (directory or single artifact).
        new_build: PathBuf,
        /// The previous version's `.cavssig`.
        #[arg(long)]
        against: PathBuf,
        /// Only print entries that changed.
        #[arg(long)]
        changes_only: bool,
        /// Flag archive/high-entropy files (zip, gzip, zstd, 7z, …) whose
        /// shape defeats block-level patching, with cost estimates.
        #[arg(long)]
        detect_compressed_blobs: bool,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Produce a deterministic offline reconstruction plan (`.cavsplan`)
    /// describing how to rebuild the new build from the old one.
    DiffPlan {
        /// The old build (file or directory). Optional with --old-signature.
        old: Option<PathBuf>,
        /// The new build (file or directory).
        new: PathBuf,
        /// Output `.cavsplan` path.
        #[arg(short, long)]
        out: PathBuf,
        /// Diff against a `.cavssig` instead of the old bytes.
        #[arg(long)]
        old_signature: Option<PathBuf>,
        /// Emit an analysis-only plan (ops and estimates, no payload).
        #[arg(long)]
        analysis: bool,
        /// Signature block size in KiB when signing the old build here.
        #[arg(long, default_value_t = 64)]
        block_kib: u32,
        /// zstd level for the plan's inline payload.
        #[arg(long, default_value_t = 19)]
        zstd_level: i32,
        /// Also write a human-readable Markdown report.
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Apply a `.cavsplan` locally: artifact plans write `<out>.part` then
    /// rename; directory plans stage, verify, journal and commit per file.
    /// A failed apply never leaves corrupt output.
    Apply {
        /// The old build (file or directory).
        #[arg(long)]
        old: Option<PathBuf>,
        /// The `.cavsplan` to execute.
        #[arg(long)]
        plan: Option<PathBuf>,
        /// Output path (omit with --inplace).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Update the old install in place (directory plans).
        #[arg(long)]
        inplace: bool,
        /// Re-verify every output hash after the apply commits.
        #[arg(long)]
        verify: bool,
        /// Delete files the plan marks as removed (managed deletions).
        #[arg(long)]
        delete_removed_files: bool,
        /// Verify the old source against the plan's recorded hash first.
        #[arg(long)]
        check_old: bool,
        /// Resume an interrupted directory apply from its journal.
        #[arg(long)]
        resume: Option<PathBuf>,
        /// Machine-readable JSON stats on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Verify an installed artifact or directory against a `.cavssig` or a
    /// manifest; reports MODIFIED/MISSING/EXTRA and exits non-zero on
    /// mismatch.
    VerifyInstall {
        /// The installed build (file or directory).
        target: PathBuf,
        /// Verify against this `.cavssig`.
        #[arg(long)]
        signature: Option<PathBuf>,
        /// Verify against this manifest's recorded SHA-256 digests.
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Tolerate files not covered by the signature (mods, saves).
        #[arg(long)]
        allow_extra_files: bool,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Identify any CAVS file (.cavs, .cavssig, .cavsplan, .cavspatch,
    /// manifest, bootstrap) and print its headline facts.
    File {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List the entries inside a CAVS file (signatures, plans, containers,
    /// manifests).
    Ls {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Generate an optimized pairwise sidecar (`.cavspatch` v2): per-file
    /// strategy selection (copy-old/renames, CAVS plan ops, bsdiff,
    /// xdelta3, recompressed full data), every candidate measured, the
    /// smallest kept. Serves exactly one old→new pair — generate only for
    /// hot pairs (`cavs patch-policy`); the pair count grows O(N²).
    OptimizePatch {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// auto (measure candidates per file) | plan | bsdiff | xdelta3 | full.
        #[arg(long, default_value = "auto")]
        algo: String,
        /// auto (best of zstd-19/brotli-9 per section) | zstd-N | brotli-N | none.
        #[arg(long, default_value = "auto")]
        compression: String,
        /// Write a per-file strategy report (Markdown) to this path.
        #[arg(long)]
        explain_strategies: Option<PathBuf>,
        /// Output `.cavspatch` path.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Apply a `.cavspatch` sidecar (v1 or v2): staged, journaled,
    /// hash-verified; nothing is committed on any mismatch.
    ApplyPatch {
        /// Old build (must match what the patch was generated against).
        #[arg(long)]
        old: PathBuf,
        /// The `.cavspatch` file.
        #[arg(long)]
        patch: PathBuf,
        /// Output path (file for artifact patches, directory for directory
        /// patches; may equal --old for in-place).
        #[arg(short, long)]
        out: PathBuf,
        /// Refuse strategies whose estimated peak memory exceeds this
        /// budget (e.g. 128MiB); the .cavsplan route always fits small
        /// budgets.
        #[arg(long)]
        memory_budget: Option<String>,
        /// Delete files the patch marks as removed (managed deletions).
        #[arg(long)]
        delete_removed_files: bool,
        /// Verify old files against their recorded hashes before use.
        #[arg(long)]
        check_old: bool,
    },
    /// Choose the best delivery route for one client state: no-op,
    /// chunks/hybrid, offline plan, optimized sidecar, bootstrap or full
    /// download — scored under a device profile.
    RoutePlan {
        /// The installed old version (omit for a fresh install).
        #[arg(long)]
        installed: Option<PathBuf>,
        /// The target build (file or directory).
        #[arg(long)]
        new: PathBuf,
        /// A pre-generated `.cavsplan` for this pair (exact size).
        #[arg(long)]
        plan: Option<PathBuf>,
        /// A pre-generated `.cavspatch` for this pair (exact size).
        #[arg(long)]
        patch: Option<PathBuf>,
        /// A bootstrap artifact for the target (exact size).
        #[arg(long)]
        bootstrap: Option<PathBuf>,
        /// Device profile the routes are scored under.
        #[arg(long, value_enum, default_value_t = route_plan::ClientProfile::Default)]
        profile: route_plan::ClientProfile,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Decide which old→new pairs deserve an optimized sidecar under a
    /// hot-pair policy (previous, latest-stable, top-installed, pins) —
    /// never all O(N²) pairs.
    PatchPolicy {
        /// Ordered, comma-separated version list; the last one is the
        /// release target (e.g. v1,v2,v3-beta,v4).
        #[arg(long)]
        versions: String,
        /// JSON map of installed shares, e.g. {"v1":0.12,"v3":0.55}.
        #[arg(long)]
        distribution: Option<PathBuf>,
        /// TOML policy file ([optimized_patches] table); defaults apply
        /// without one.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Publish a directory build in one pass: container + signature +
    /// offline plan + optimized sidecar vs the previous release, with a
    /// preview (renames, compressed-blob warnings) first.
    PublishDir {
        /// The exported build folder.
        build: PathBuf,
        /// The previous build directory or its `.cavssig`.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// Where the release files are written.
        #[arg(long)]
        out_dir: PathBuf,
        /// auto (generate the previous→this sidecar) | off.
        #[arg(long, default_value = "auto")]
        optimize_patches: String,
        /// Exclude entries matching this glob (repeatable; merged with
        /// `.cavsignore`).
        #[arg(long)]
        ignore: Vec<String>,
        /// zstd level for the container's stored chunks.
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
        /// Sign the packed content with this Ed25519 secret key file.
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// Only print the preview; write nothing.
        #[arg(long)]
        preview: bool,
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
enum SignatureAction {
    /// Export a `.cavssig` from a `.cavs` container (default) or a raw
    /// file/directory (--raw).
    Export {
        /// A .cavs file, or with --raw any file or directory.
        input: PathBuf,
        /// Treat the input as a raw artifact/directory instead of a .cavs.
        #[arg(long)]
        raw: bool,
        /// Block size in KiB (64 is the empirical sweet spot for delta scanning).
        #[arg(long, default_value_t = 64)]
        block_kib: u32,
        /// Output .cavssig path.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Print a signature's layout, chunk profile and hash counts.
    Inspect {
        input: PathBuf,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// List every entry recorded in a signature.
    Ls {
        input: PathBuf,
        /// Machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Recompute every block hash of a source and compare.
    Verify {
        input: PathBuf,
        /// The artifact or directory the signature claims to describe.
        #[arg(long)]
        against: PathBuf,
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
    /// Interrupted-apply matrix (v0.8.0): SIGKILL real `cavs apply` runs
    /// at ramping delays, assert no torn files, prove journaled resume;
    /// plus corrupt-plan / corrupt-old / garbage-staging cases.
    ApplyRecovery {
        /// Old build directory.
        #[arg(long)]
        old: PathBuf,
        /// New build directory.
        #[arg(long)]
        new: PathBuf,
        /// Write apply-recovery.json here.
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
    /// Compare CAVS against a block-based delta patching model on a real
    /// old/new pair (v0.6.0). Uses xdelta3/bsdiff too when on PATH.
    Delta {
        /// Old version (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New version (file or directory — same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Directory for delta-comparison.{json,md}.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Measure compression algorithms on a payload (v0.6.0): zstd always,
    /// Brotli with `--features brotli-bench`. Defaults stay zstd-3.
    Compression {
        /// The payload to measure.
        #[arg(long)]
        input: PathBuf,
        /// Comma-separated algos, e.g. zstd-1,zstd-3,zstd-19,brotli-1,brotli-9.
        #[arg(long, default_value = "zstd-1,zstd-3,zstd-9,zstd-19,brotli-1,brotli-9")]
        algos: String,
        /// Write the report as markdown to this path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Benchmark the external `butler` binary's *complete* patch pipeline
    /// (v0.8.0): diff, rediff --rediff-quality 9, apply and verify for
    /// both the default and the optimized patch, with times and peak RSS.
    ButlerFull {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Path to the butler binary (default: `butler` on PATH).
        #[arg(long, default_value = "butler")]
        butler_bin: String,
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
    },
    /// The proof report (v0.8.0): every CAVS route (chunks, plan, sidecar,
    /// auto-route) and the full external butler pipeline on one pair, one
    /// table, honest win/loss verdicts. CAVS apply times/RSS measured via
    /// real subprocesses, all outputs verified byte-identical.
    FullPipeline {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Also run the external butler harness with this binary.
        #[arg(long)]
        butler_bin: Option<String>,
        /// Include butler rediff (optimized patch) in the butler run.
        #[arg(long, default_value_t = true)]
        include_rediff: bool,
        /// Include bsdiff/xdelta3 pairwise proxies.
        #[arg(long)]
        include_pairwise: bool,
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
    },
    /// Benchmark the external `butler` binary's offline diff/apply/verify
    /// pipeline on a real old/new pair (v0.7.0). Measures the default
    /// patch only; `bench butler-full` also measures the optimized one.
    ButlerOffline {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Path to the butler binary (default: `butler` on PATH).
        #[arg(long, default_value = "butler")]
        butler_bin: String,
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
    },
    /// Approximate the optimized pairwise patch class (bsdiff/xdelta3 +
    /// recompression) with transparent local tools (v0.7.0). Results are
    /// always labeled as a proxy.
    PairwiseProxy {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Comma-separated delta tools.
        #[arg(long, default_value = "bsdiff,xdelta3")]
        algos: String,
        /// Comma-separated recompressions (zstd-N, brotli-N, none).
        #[arg(long, default_value = "zstd-19,brotli-9")]
        compression: String,
        /// Results directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Compare every delivery route for one old→new transition (v0.7.0):
    /// full downloads, CAVS chunk/hybrid, CAVS offline plan, butler
    /// offline, pairwise proxies. Missing tools are skipped, not fatal.
    Routes {
        /// Old build (file or directory).
        #[arg(long)]
        old: PathBuf,
        /// New build (same kind as --old).
        #[arg(long)]
        new: PathBuf,
        /// Also run the external butler harness with this binary.
        #[arg(long)]
        butler_bin: Option<String>,
        /// Include bsdiff/xdelta3 optimized pairwise proxies.
        #[arg(long)]
        include_pairwise_proxy: bool,
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
    },
    /// Generate a deterministic synthetic *directory* build pair
    /// (Build_v1/, Build_v2/) with modified, new, deleted and renamed
    /// files — the shapes that matter for per-file update delivery.
    GenDir {
        /// Output dataset directory.
        #[arg(long)]
        out: PathBuf,
        /// Approximate build size, e.g. 128MiB.
        #[arg(long, default_value = "128MiB")]
        size: String,
        /// PRNG seed (same seed + size => identical trees).
        #[arg(long, default_value_t = 5)]
        seed: u64,
    },
    /// Many-version stream (v0.7.0): v1→vN with ~3% drift per release;
    /// compares CAVS store-once delivery against pairwise patch storage
    /// for adjacent updates, long jumps and reinstalls.
    VersionStream {
        /// Results directory.
        #[arg(long)]
        out: PathBuf,
        /// Size of each version.
        #[arg(long, default_value = "32MiB")]
        size: String,
        /// Number of releases in the stream.
        #[arg(long, default_value_t = 10)]
        versions: usize,
        /// PRNG seed.
        #[arg(long, default_value_t = 5)]
        seed: u64,
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
        /// Also write per-asset `chunk-map.json` files (v0.6.0): everything
        /// a static client needs to plan a fetch without a smart server.
        #[arg(long)]
        static_plans: bool,
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
            against_signature,
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
                against_signature,
            };
            if raw {
                pack::pack_raw(&inputs, &output, &opts)
            } else {
                pack::pack_video(&inputs, &output, &opts)
            }
        }
        Command::PackDir {
            input,
            output,
            profile,
            no_compress,
            zstd_level,
            sign_key,
            ignore,
        } => pack_dir::pack_dir(
            &input,
            &output,
            &pack_dir::PackDirOptions {
                profile,
                compress: !no_compress,
                zstd_level,
                sign_key,
                ignore,
            },
        ),
        Command::Signature { action } => match action {
            SignatureAction::Export {
                input,
                raw,
                block_kib,
                output,
            } => signature_cmd::export(&input, raw, block_kib.max(1) * 1024, &output),
            SignatureAction::Inspect { input, json } => signature_cmd::inspect(&input, json),
            SignatureAction::Ls { input, json } => signature_cmd::ls(&input, json),
            SignatureAction::Verify { input, against } => signature_cmd::verify(&input, &against),
        },
        Command::Preview {
            new_build,
            against,
            changes_only,
            detect_compressed_blobs,
            json,
        } => preview::preview(
            &new_build,
            &against,
            changes_only,
            detect_compressed_blobs,
            json,
        ),
        Command::DiffPlan {
            old,
            new,
            out,
            old_signature,
            analysis,
            block_kib,
            zstd_level,
            report,
        } => diff_plan::diff_plan(&diff_plan::DiffPlanArgs {
            old: old.as_deref(),
            old_signature: old_signature.as_deref(),
            new: &new,
            out: &out,
            analysis,
            block_kib,
            zstd_level,
            report: report.as_deref(),
        }),
        Command::Apply {
            old,
            plan,
            out,
            inplace,
            verify,
            delete_removed_files,
            check_old,
            resume,
            json,
        } => apply_cmd::apply(&apply_cmd::ApplyArgs {
            old: old.as_deref(),
            plan: plan.as_deref(),
            out: out.as_deref(),
            inplace,
            verify,
            delete_removed: delete_removed_files,
            check_old,
            resume: resume.as_deref(),
            json,
        }),
        Command::VerifyInstall {
            target,
            signature,
            manifest,
            allow_extra_files,
            json,
        } => verify_install::verify_install(
            &target,
            signature.as_deref(),
            manifest.as_deref(),
            allow_extra_files,
            json,
        ),
        Command::File { input, json } => inspect_cmd::file_info(&input, json),
        Command::Ls { input, json } => inspect_cmd::ls(&input, json),
        Command::OptimizePatch {
            old,
            new,
            algo,
            compression,
            explain_strategies,
            out,
        } => {
            let report = patch_v2::generate(
                &old,
                &new,
                &patch_v2::GenerateOptions { algo, compression },
                &out,
            )?;
            println!(
                "sidecar : {} ({} for {} → {}, {} ms)",
                out.display(),
                report::human_bytes(report.patch_bytes),
                report::human_bytes(report.old_total_size),
                report::human_bytes(report.new_total_size),
                report.gen_ms,
            );
            println!(
                "files   : {} copy-old ({} renames) · {} plan-ops · {} bsdiff · {} xdelta3 · {} full-data · {} deletions",
                report.files_copy_old,
                report.renames_detected,
                report.files_plan_ops,
                report.files_bsdiff,
                report.files_xdelta3,
                report.files_full_data,
                report.deleted,
            );
            if !report.skipped_tools.is_empty() {
                println!(
                    "note    : candidates not measured (tool missing): {}",
                    report.skipped_tools.join(", ")
                );
            }
            println!(
                "note    : sidecars serve exactly this old→new pair; generate them only \
                 for hot pairs (cavs patch-policy)"
            );
            if let Some(path) = explain_strategies {
                std::fs::write(&path, patch_v2::explain_markdown(&report))?;
                println!("report  : {}", path.display());
            }
            Ok(())
        }
        Command::ApplyPatch {
            old,
            patch,
            out,
            memory_budget,
            delete_removed_files,
            check_old,
        } => {
            let magic = {
                let mut f = std::fs::File::open(&patch)?;
                let mut m = [0u8; 8];
                use std::io::Read as _;
                let _ = f.read(&mut m)?;
                m
            };
            if magic == *b"CAVSPCH1" {
                optimize_patch::apply(&old, &patch, &out)
            } else {
                let budget = memory_budget
                    .as_deref()
                    .map(synth::parse_size_pub)
                    .transpose()?;
                let stats = patch_v2::apply(
                    &patch,
                    &old,
                    &out,
                    &patch_v2::ApplyV2Options {
                        delete_removed: delete_removed_files,
                        memory_budget_bytes: budget,
                        check_old,
                    },
                )?;
                println!(
                    "apply   : OK — {} ({} written, {} no-op, {} deleted, {} ms)",
                    out.display(),
                    stats.files_written,
                    stats.files_noop,
                    stats.deleted,
                    stats.elapsed_ms,
                );
                Ok(())
            }
        }
        Command::RoutePlan {
            installed,
            new,
            plan,
            patch,
            bootstrap,
            profile,
            json,
        } => route_plan::route_plan(&route_plan::RoutePlanArgs {
            installed: installed.as_deref(),
            new: &new,
            plan: plan.as_deref(),
            patch: patch.as_deref(),
            bootstrap: bootstrap.as_deref(),
            profile,
            json,
        }),
        Command::PatchPolicy {
            versions,
            distribution,
            config,
            json,
        } => patch_policy::run(&versions, distribution.as_deref(), config.as_deref(), json),
        Command::PublishDir {
            build,
            previous,
            out_dir,
            optimize_patches,
            ignore,
            zstd_level,
            sign_key,
            preview,
        } => publish_dir::publish_dir(&publish_dir::PublishArgs {
            build: &build,
            previous: previous.as_deref(),
            out_dir: &out_dir,
            optimize_patches,
            ignore,
            zstd_level,
            sign_key: sign_key.as_deref(),
            preview_only: preview,
        }),
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
            StoreAction::Export { out, static_plans } => store::export(&dir, &out, static_plans),
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
            TestAction::ApplyRecovery { old, new, out } => {
                test_recovery::run(&old, &new, out.as_deref())
            }
        },
        Command::Bench { action } => match action {
            BenchAction::Gen { out, size, seed } => synth::generate(&out, &size, seed),
            BenchAction::GenDir { out, size, seed } => synth::generate_dir(&out, &size, seed),
            BenchAction::Suite { dataset, out } => synth::suite(&dataset, &out),
            BenchAction::Delta { old, new, out } => bench_delta::bench(&old, &new, out.as_deref()),
            BenchAction::Compression { input, algos, out } => {
                bench_compression::bench(&input, &algos, out.as_deref())
            }
            BenchAction::ButlerOffline {
                old,
                new,
                butler_bin,
                out,
            } => bench_butler::bench(&old, &new, &butler_bin, &out),
            BenchAction::ButlerFull {
                old,
                new,
                butler_bin,
                out,
            } => bench_butler_full::bench(&old, &new, &butler_bin, &out),
            BenchAction::FullPipeline {
                old,
                new,
                butler_bin,
                include_rediff,
                include_pairwise,
                out,
            } => bench_pipeline::bench(&bench_pipeline::PipelineArgs {
                old: &old,
                new: &new,
                butler_bin: butler_bin.as_deref(),
                include_rediff,
                include_pairwise,
                out: &out,
            }),
            BenchAction::PairwiseProxy {
                old,
                new,
                algos,
                compression,
                out,
            } => bench_pairwise::bench(&old, &new, &algos, &compression, out.as_deref()),
            BenchAction::Routes {
                old,
                new,
                butler_bin,
                include_pairwise_proxy,
                out,
            } => bench_routes::bench(&bench_routes::RoutesArgs {
                old: &old,
                new: &new,
                butler_bin: butler_bin.as_deref(),
                include_pairwise_proxy,
                out: &out,
            }),
            BenchAction::VersionStream {
                out,
                size,
                versions,
                seed,
            } => bench_versions::bench(&out, &size, versions, seed),
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
