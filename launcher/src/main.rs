//! LogicLab self-extracting launcher.
//!
//! GDExtension libraries must exist as real files on disk (Windows' LoadLibrary
//! needs a filesystem path, and .gdextension is parsed before any game code runs),
//! so a Godot game using one cannot be a single literal executable. This launcher
//! makes it *look* like one: the game exe + bridge DLL are appended to this binary
//! as a trailing ZIP payload. On first run they are extracted to
//! %LOCALAPPDATA%\LogicLab\runtime, then the game is started. Later runs find the
//! stamp matching and skip extraction entirely (fast path).
//!
//! Bundle layout:  [this exe][zip payload][u64 LE payload length]

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Trailing 8-byte little-endian length of the appended zip payload.
const TRAILER_LEN: u64 = 8;
const APP_DIR: &str = "LogicLab";
const STAMP_FILE: &str = ".payload-stamp";
const GAME_EXE: &str = "LogicLab.exe";
const VERSION_ENTRY: &str = "version.txt";

fn main() {
    if let Err(e) = run() {
        eprintln!("LogicLab launcher: {e}");
        eprintln!("If you ran this file directly from a build directory, it has no payload.");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let exe_path = env::current_exe()?;
    let mut f = File::open(&exe_path)?;
    let total = f.metadata()?.len();
    if total <= TRAILER_LEN {
        return Err(bad("file too small to contain a payload"));
    }

    // Read the trailing payload length.
    f.seek(SeekFrom::End(-(TRAILER_LEN as i64)))?;
    let mut len_bytes = [0u8; 8];
    f.read_exact(&mut len_bytes)?;
    let payload_len = u64::from_le_bytes(len_bytes);

    let payload_start = total
        .checked_sub(TRAILER_LEN)
        .and_then(|v| v.checked_sub(payload_len))
        .ok_or_else(|| bad("payload length exceeds file size"))?;

    // Load the payload (bounded by the archive we appended ourselves).
    f.seek(SeekFrom::Start(payload_start))?;
    let mut payload = vec![0u8; payload_len as usize];
    f.read_exact(&mut payload)?;
    drop(f);

    let runtime = runtime_dir();
    let stamp_path = runtime.join(STAMP_FILE);
    let want_stamp = payload_stamp(&payload)?;

    let up_to_date = fs::read_to_string(&stamp_path)
        .map(|s| s.trim() == want_stamp)
        .unwrap_or(false);

    if !up_to_date {
        println!("LogicLab: first run, unpacking runtime...");
        if runtime.exists() {
            fs::remove_dir_all(&runtime)?;
        }
        fs::create_dir_all(&runtime)?;
        extract_zip(&payload, &runtime)?;
        fs::write(&stamp_path, &want_stamp)?;
    }

    let game = runtime.join(GAME_EXE);
    if !game.is_file() {
        return Err(bad(&format!("payload did not contain {GAME_EXE}")));
    }

    // Forward any CLI args to the game, and run it from the runtime dir so
    // relative asset paths resolve next to the executable.
    let args: Vec<String> = env::args().skip(1).collect();
    Command::new(&game)
        .args(&args)
        .current_dir(&runtime)
        .spawn()?;

    Ok(())
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Where the runtime is unpacked: %LOCALAPPDATA%\LogicLab\runtime
fn runtime_dir() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    base.join(APP_DIR).join("runtime")
}

/// A cheap identity for the payload so re-runs skip extraction.
/// Prefers a `version.txt` entry inside the archive; falls back to a hash of
/// the payload bytes.
fn payload_stamp(payload: &[u8]) -> io::Result<String> {
    if let Ok(mut zip) = zip::ZipArchive::new(io::Cursor::new(payload)) {
        if let Ok(mut v) = zip.by_name(VERSION_ENTRY) {
            let mut s = String::new();
            v.read_to_string(&mut s)?;
            let t = s.trim();
            if !t.is_empty() {
                return Ok(t.to_string());
            }
        }
    }
    Ok(format!("len-{:x}", fnv1a(payload)))
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn extract_zip(data: &[u8], dest: &Path) -> io::Result<()> {
    let mut zip = zip::ZipArchive::new(io::Cursor::new(data))
        .map_err(|e| bad(&format!("payload is not a valid zip: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)
            .map_err(|e| bad(&format!("zip entry {i}: {e}")))?;
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue, // refuse path traversal
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut w = File::create(&out)?;
            io::copy(&mut entry, &mut w)?;
        }
    }
    Ok(())
}