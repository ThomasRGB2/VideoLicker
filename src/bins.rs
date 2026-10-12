//! Tool discovery / install helpers and small OS integrations.

use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// Keep child processes from flashing a console window on Windows.
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

pub fn ytdlp_name() -> &'static str {
    if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" }
}

pub fn ffmpeg_name() -> &'static str {
    if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }
}

/// Per-app data dir, e.g. %APPDATA%\TubeGrab on Windows (created as needed).
pub fn app_dir() -> PathBuf {
    let d = match (cfg!(windows), var("APPDATA")) {
        (true, Some(a)) => PathBuf::from(a).join("TubeGrab"),
        _ => PathBuf::from(var("HOME").unwrap_or_else(|| ".".into()))
            .join(".local/share")
            .join("tubegrab"),
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Where we keep our own yt-dlp/ffmpeg copies.
pub fn bin_dir() -> PathBuf {
    let b = app_dir().join("bin");
    let _ = std::fs::create_dir_all(&b);
    b
}

/// The user's Downloads folder (falls back to a folder inside app_dir).
pub fn default_save_dir() -> PathBuf {
    let home = if cfg!(windows) { var("USERPROFILE") } else { var("HOME") };
    let d = home
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| app_dir().join("downloads"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Locate a tool by file name along PATH.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let pv = std::env::var_os("PATH")?;
    std::env::split_paths(&pv)
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

/// Where `winget` drops per-user tools.
pub fn winget_links() -> Option<PathBuf> {
    var("LOCALAPPDATA").map(|l| {
        PathBuf::from(l).join("Microsoft").join("WinGet").join("Links")
    })
}

/// Lookup order: PATH -> winget links -> our own bin dir.
fn find_tool(name: &str) -> Option<PathBuf> {
    if let Some(p) = find_on_path(name) {
        return Some(p);
    }
    if let Some(c) = winget_links().map(|l| l.join(name)) {
        if c.is_file() {
            return Some(c);
        }
    }
    let c = bin_dir().join(name);
    c.is_file().then_some(c)
}

pub fn find_ytdlp() -> Option<PathBuf> {
    find_tool(ytdlp_name())
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    find_tool(ffmpeg_name())
}

/// Download the latest yt-dlp single-file release into bin/.
pub fn install_ytdlp() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Err("automatic install is Windows-only; install yt-dlp with your package manager".into());
    }
    let dest = bin_dir().join("yt-dlp.exe");
    let tmp = app_dir().join("yt-dlp.exe.new");
    let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

    let resp = ureq::get(url).call().map_err(|e| format!("download failed: {}", e))?;
    let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
    drop(f);
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("could not move yt-dlp into place: {}", e))?;
    Ok(dest)
}

/// Download the ffmpeg "essentials" build and extract ffmpeg.exe + ffprobe.exe into bin/.
pub fn install_ffmpeg() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Err("automatic install is Windows-only; install ffmpeg with your package manager".into());
    }
    let bindir = bin_dir();
    let zip_path = app_dir().join("ffmpeg.zip");
    let url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";

    let resp = ureq::get(url).call().map_err(|e| format!("download failed: {}", e))?;
    let mut f = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
    io::copy(&mut resp.into_reader(), &mut f).map_err(|e| e.to_string())?;
    drop(f);

    let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut ar = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    let mut found: Option<PathBuf> = None;

    for i in 0..ar.len() {
        let mut e = ar.by_index(i).map_err(|e| e.to_string())?;
        if !e.is_file() {
            continue;
        }
        // Flatten to the bare file name (also avoids any path traversal in the archive).
        let base = e.name().rsplit(['/', '\\']).next().unwrap_or("").to_string();
        if base != "ffmpeg.exe" && base != "ffprobe.exe" {
            continue;
        }
        let out = bindir.join(&base);
        let mut of = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        io::copy(&mut e, &mut of).map_err(|e| e.to_string())?;
        if base == "ffmpeg.exe" {
            found = Some(out);
        }
    }

    let _ = std::fs::remove_file(&zip_path);
    found.ok_or_else(|| "ffmpeg.exe not present in that build".to_string())
}

/// Open a folder in the OS file manager. Best-effort.
pub fn open_folder(p: &Path) {
    let _ = std::fs::create_dir_all(p);
    let prog = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(prog).arg(p).spawn();
}

/// Native folder picker starting at `start`. Returns the chosen dir, or `start` if cancelled.
/// Blocking: call from a worker thread.
pub fn pick_folder(start: PathBuf) -> PathBuf {
    let mut cmd;
    if cfg!(windows) {
        cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-STA", "-Command"]).arg(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }",
        );
    } else if cfg!(target_os = "macos") {
        cmd = Command::new("osascript");
        cmd.args(["-e", "POSIX path of (choose folder)"]);
    } else {
        cmd = Command::new("zenity");
        cmd.args(["--file-selection", "--directory"]);
    }
    hide_console(&mut cmd);
    if let Ok(out) = cmd.output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    start
}

pub fn find_deno() -> Option<PathBuf> {
    if let Some(p) = find_on_path("deno.exe") {
        return Some(p);
    }

    if let Some(profile) = var("USERPROFILE") {
        let p = PathBuf::from(profile)
            .join(".deno")
            .join("bin")
            .join("deno.exe");
        if p.exists() {
            return Some(p);
        }
    }

    if let Some(local) = var("LOCALAPPDATA") {
        let p = PathBuf::from(local)
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("deno.exe");
        if p.exists() {
            return Some(p);
        }
    }

    None
}

pub fn install_deno() -> Result<(), String> {
    run_winget(&[
        "install", "--exact", "--id", "DenoLand.Deno",
        "--accept-package-agreements",
        "--accept-source-agreements",
    ])
}

pub fn update_deno() -> Result<(), String> {
    run_winget(&[
        "upgrade", "--exact", "--id", "DenoLand.Deno",
        "--accept-package-agreements",
        "--accept-source-agreements",
    ])
}

fn run_winget(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("winget")
        .args(args)
        .status()
        .map_err(|e| format!("Could not start winget: {e}"))?;

    if status.success() {
        return Ok(());
    }

    // WinGet: no applicable update found.
    // Treat this as success for upgrade operations.
    if args.first() == Some(&"upgrade")
        && status.code() == Some(-1978335189)
    {
        return Ok(());
    }

    Err(format!("winget exited with status: {status}"))
}