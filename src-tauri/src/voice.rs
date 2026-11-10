//! Ghost on the PC (owner 2026-11-10, D052): push-to-talk. The page records the microphone while the button or hotkey
//! is held and hands over a 16 kHz WAV; whisper.cpp (MIT, official release, run only while transcribing, then it exits)
//! turns it into text. What was said then goes to the hub's Ghost (`/v1/voice`), or is handled here when it's about
//! this PC. Both downloads are pinned by SHA-256, so only the exact files tested here ever run.
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

type R<T> = Result<T, String>;

const BIN_URL: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip";
const BIN_SHA: &str = "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
const MODEL_SHA: &str = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const MAX_WAV: usize = 16_000 * 2 * 60 + 44; // one minute of 16 kHz mono 16-bit

fn dir() -> R<PathBuf> {
    Ok(crate::standby::data_dir().ok_or("no data folder")?.join("whisper"))
}
fn cli() -> R<PathBuf> {
    Ok(dir()?.join("Release").join("whisper-cli.exe"))
}
fn model() -> R<PathBuf> {
    Ok(dir()?.join("ggml-base.en.bin"))
}

/// Downloads `url` to `to`, refusing anything over the size cap or with a different SHA-256.
fn fetch(url: &str, sha: &str, to: &PathBuf) -> R<()> {
    let r = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).build().get(url).call().map_err(|e| format!("Download failed: {e}"))?;
    let tmp = to.with_extension("part");
    let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let (mut h, mut rd, mut buf, mut n) = (Sha256::new(), r.into_reader().take(MAX_DOWNLOAD + 1), vec![0u8; 1 << 16], 0u64);
    loop {
        let k = rd.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if k == 0 {
            break;
        }
        n += k as u64;
        h.update(&buf[..k]);
        f.write_all(&buf[..k]).map_err(|e| e.to_string())?;
    }
    drop(f);
    let got = format!("{:x}", h.finalize());
    if n > MAX_DOWNLOAD || got != sha {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("The download from {url} wasn't the expected file, so it was thrown away."));
    }
    std::fs::rename(&tmp, to).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn voice_status() -> R<serde_json::Value> {
    Ok(serde_json::json!({ "installed": cli()?.is_file() && model()?.is_file() }))
}

/// One-time setup (~160 MB): the speech program and its English model.
#[tauri::command(async)]
pub fn voice_install() -> R<()> {
    let d = dir()?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    if !cli()?.is_file() {
        let zip = d.join("whisper-bin-x64.zip");
        fetch(BIN_URL, BIN_SHA, &zip)?;
        let tar = std::env::var("SystemRoot").map(|r| format!(r"{r}\System32\tar.exe")).unwrap_or_else(|_| "tar".into());
        let (zs, ds) = (zip.to_string_lossy().to_string(), d.to_string_lossy().to_string());
        crate::util::run(&tar, &["-xf", &zs, "-C", &ds], None, Duration::from_secs(120))?;
        let _ = std::fs::remove_file(&zip);
        if !cli()?.is_file() {
            return Err("The speech program didn't unpack.".into());
        }
    }
    if !model()?.is_file() {
        fetch(MODEL_URL, MODEL_SHA, &model()?)?;
    }
    Ok(())
}

/// Speech to text. `hint` = the team's own words (workspace and project names) so Whisper spells them right.
#[tauri::command(async)]
pub fn voice_transcribe(wav: String, hint: String) -> R<String> {
    let bytes = crate::hub_client::b64_decode(&wav)?;
    if bytes.len() < 44 || bytes.len() > MAX_WAV || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("That recording couldn't be used (too long or not audio).".into());
    }
    if !cli()?.is_file() || !model()?.is_file() {
        return Err("Set up voice first: Settings › Ghost voice › Download.".into());
    }
    let file = std::env::temp_dir().join(format!("ptcc-ghost-{}-{}.wav", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    std::fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    let prompt: String = format!("Ghost commands. {}", hint.chars().filter(|c| c.is_alphanumeric() || " ,.'-".contains(*c)).take(300).collect::<String>());
    let (m, f) = (model()?.to_string_lossy().to_string(), file.to_string_lossy().to_string());
    let threads = std::thread::available_parallelism().map(|n| n.get().clamp(2, 8)).unwrap_or(4).to_string();
    let out = crate::util::run(&cli()?.to_string_lossy(), &["-m", &m, "-f", &f, "-nt", "-np", "-t", &threads, "--prompt", &prompt], None, Duration::from_secs(60));
    let _ = std::fs::remove_file(&file);
    let text = out?.stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    // Whisper writes these for silence and noise.
    let blank = text.is_empty() || ["[BLANK_AUDIO]", "(silence)", "[ Silence ]", "[Music]", "(music)"].iter().any(|b| text.eq_ignore_ascii_case(b));
    Ok(if blank { String::new() } else { text })
}

#[cfg(test)]
mod tests {
    #[test]
    fn pins_are_full_sha256() {
        for s in [super::BIN_SHA, super::MODEL_SHA] {
            assert!(s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
}
