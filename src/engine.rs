//! Background download engine: shared types + the worker thread.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::bins;

#[derive(Clone, Copy, PartialEq)]
pub enum Quality {
    Best,
    P1080,
    P720,
    Mp3,
}

impl Quality {
    pub fn label(&self) -> &'static str {
        match self {
            Quality::Best => "Best (mp4)",
            Quality::P1080 => "1080p",
            Quality::P720 => "720p",
            Quality::Mp3 => "MP3",
        }
    }

    /// yt-dlp format/output flags for this quality.
    pub fn args(&self) -> Vec<&'static str> {
        match self {
            Quality::Best => vec![
            "-f", "bestvideo*+bestaudio/best",
            "--merge-output-format", "mp4",
        ],
        Quality::P1080 => vec![
            "-f", "bestvideo*[height<=1080]+bestaudio/best",
            "--merge-output-format", "mp4",
        ],
        Quality::P720 => vec![
            "-f", "bestvideo*[height<=720]+bestaudio/best",
            "--merge-output-format", "mp4",
        ],
        Quality::Mp3 => vec![
            "-f", "bestaudio/best",
            "-x", "--audio-format", "mp3",
        ],
        }
    }
}

fn cookie_args(c: &Option<String>) -> Vec<String> {
    match c {
        Some(b) => vec!["--cookies-from-browser".into(), b.clone()],
        None => vec![],
    }
}





#[derive(Clone)]
pub enum JobStatus {
    Queued,
    FetchingMeta,
    Downloading { pct: f64 },
    Done(PathBuf),
    Error(String),
}

pub struct Job {
    pub id: usize,
    pub url: String,
    pub title: String,
    pub status: JobStatus,
}

/// Events the worker sends to the UI.
pub enum Ev {
    Meta { id: usize, title: String },
    Progress { id: usize, pct: f64 },
    Done { id: usize, path: PathBuf },
    Err { id: usize, msg: String },
    YtdlpStatus(Option<String>),
    FfmpegStatus(Option<String>),
    DenoStatus(Option<String>),
    Log(String),
}

/// Commands the UI sends to the worker.
pub enum Cmd {
    Enqueue { id: usize, url: String, quality: Quality, save_dir: PathBuf, cookies: Option<String> },
    Cancel(usize),
    CheckTools,
    InstallYtdlp,
    InstallFfmpeg,
    InstallDeno,
    UpdateDeno,
    Quit,
}

type Handle = Arc<Mutex<Option<Child>>>;
type ToolPath = Arc<Mutex<Option<PathBuf>>>;

/// Spawn the background engine. Returns (command sender, event receiver).
pub fn spawn_worker(ytdlp: Option<PathBuf>, ffmpeg: Option<PathBuf>) -> (Sender<Cmd>, Receiver<Ev>) {
    let (tx, rx) = channel::<Cmd>();
    let (etx, erx) = channel::<Ev>();
    let ytdlp_ref: ToolPath = Arc::new(Mutex::new(ytdlp));
    let ffmpeg_ref: ToolPath = Arc::new(Mutex::new(ffmpeg));

    std::thread::spawn(move || {
        let mut active: HashMap<usize, Handle> = HashMap::new();
        let cancelled: Arc<Mutex<HashSet<usize>>> = Arc::default();

        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Enqueue { id, url, quality, save_dir, cookies } => {
                    let y = ytdlp_ref.lock().unwrap().clone();
                    let f = ffmpeg_ref.lock().unwrap().clone();
                    let et = etx.clone();
                    let ct = Arc::clone(&cancelled);
                    let handle: Handle = Arc::new(Mutex::new(None));
                    active.insert(id, Arc::clone(&handle));
                    std::thread::spawn(move || {run_job(id, url, quality, y, f, save_dir, cookies, et, ct, handle)});
                }
                Cmd::Cancel(id) => {
                    cancelled.lock().unwrap().insert(id);
                    if let Some(h) = active.get(&id) {
                        if let Some(c) = h.lock().unwrap().as_mut() {
                            let _ = c.kill();
                        }
                    }
                    let _ = etx.send(Ev::Log(format!("Cancelling job {}", id)));
                }
                Cmd::CheckTools => {
                    let (yr, fr, et) = (Arc::clone(&ytdlp_ref), Arc::clone(&ffmpeg_ref), etx.clone());
                    std::thread::spawn(move || {
                        // Re-discover so tools installed outside the app are picked up.
                        let y = bins::find_ytdlp();
                        let f = bins::find_ffmpeg();
                        *yr.lock().unwrap() = y.clone();
                        *fr.lock().unwrap() = f.clone();
                        let _ = et.send(Ev::YtdlpStatus(tool_version(&y, false)));
                        let _ = et.send(Ev::FfmpegStatus(tool_version(&f, true)));
                        let _ = et.send(Ev::DenoStatus(deno_version()));
                    });
                }
                Cmd::InstallYtdlp => {
                    let (et, yr) = (etx.clone(), Arc::clone(&ytdlp_ref));
                    std::thread::spawn(move || match bins::install_ytdlp() {
                        Ok(p) => {
                            *yr.lock().unwrap() = Some(p.clone());
                            let _ = et.send(Ev::Log(format!("yt-dlp installed: {}", p.display())));
                            let _ = et.send(Ev::YtdlpStatus(tool_version(&Some(p), false)));
                        }
                        Err(e) => {
                            let _ = et.send(Ev::Log(format!("yt-dlp install failed: {}", e)));
                            let cur = yr.lock().unwrap().clone();
                            let _ = et.send(Ev::YtdlpStatus(tool_version(&cur, false)));
                        }
                    });
                }
                Cmd::InstallFfmpeg => {
                    let (et, fr) = (etx.clone(), Arc::clone(&ffmpeg_ref));
                    std::thread::spawn(move || match bins::install_ffmpeg() {
                        Ok(p) => {
                            *fr.lock().unwrap() = Some(p.clone());
                            let _ = et.send(Ev::Log(format!("ffmpeg installed: {}", p.display())));
                            let _ = et.send(Ev::FfmpegStatus(tool_version(&Some(p), true)));
                        }
                        Err(e) => {
                            let _ = et.send(Ev::Log(format!("ffmpeg install failed: {}", e)));
                            let cur = fr.lock().unwrap().clone();
                            let _ = et.send(Ev::FfmpegStatus(tool_version(&cur, true)));
                        }
                    });
                }
                Cmd::InstallDeno | Cmd::UpdateDeno => {
                    let install = matches!(cmd, Cmd::InstallDeno);
                    let et = etx.clone();

                    std::thread::spawn(move || {
                     let result = if install {
                     bins::install_deno()
                    } else {
                    bins::update_deno()
                      };

                    match result {
            Ok(()) => {
                let _ = et.send(Ev::Log(
                    "Deno installation/update completed.".into()
                ));
            }
            Err(e) => {
                let _ = et.send(Ev::Log(
                    format!("Deno installation/update failed: {e}")
                ));
            }
        }

        let _ = et.send(Ev::DenoStatus(deno_version()));
    });
}
                Cmd::Quit => {
                    for (id, h) in active.iter() {
                        cancelled.lock().unwrap().insert(*id);
                        if let Some(c) = h.lock().unwrap().as_mut() {
                            let _ = c.kill();
                        }
                    }
                    break;
                }
            }
        }
    });

    (tx, erx)
}

/// Run a single download job end-to-end (in its own thread).
#[allow(clippy::too_many_arguments)]
fn run_job(
    id: usize,
    url: String,
    quality: Quality,
    ytdlp: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    save: PathBuf,
    cookies: Option<String>,
    etx: Sender<Ev>,
    cancelled: Arc<Mutex<HashSet<usize>>>,
    handle: Handle,
) {
    let send = |ev: Ev| {
        let _ = etx.send(ev);
    };
    let is_cancelled = || cancelled.lock().unwrap().contains(&id);

    let Some(bin) = ytdlp else {
        send(Ev::Err { id, msg: "yt-dlp not found — hit “Install” up top".into() });
        return;
    };
    let Some(ff) = ffmpeg else {
        send(Ev::Err { id, msg: "ffmpeg is required — hit “Install” up top first".into() });
        return;
    };

    // 1) Metadata: real title + id (for the filename). Non-fatal if it fails.
    send(Ev::Meta { id, title: url.clone() });
    let (title, mut vid) = match fetch_metadata(&bin, &url, &cookies) {
        Some(t) => t,
        None => {
            send(Ev::Log(format!("#{}: metadata unavailable, using URL", id)));
            (url.clone(), String::new())
        }
    };
    if vid.is_empty() {
        vid = derive_id(&url);
    }
    send(Ev::Log(format!("#{}: {}", id, title)));
    send(Ev::Meta { id, title });

    if is_cancelled() {
        send(Ev::Err { id, msg: "Cancelled".into() });
        return;
    }

    // 2) Download.
    let _ = std::fs::create_dir_all(&save);
    let mut cmd = Command::new(&bin);
    cmd.args(quality.args());
    cmd.args(["--remote-components", "ejs:npm"]);
    cmd.arg("--ffmpeg-location").arg(ff.parent().unwrap_or_else(|| Path::new(".")));
    cmd.arg("-o").arg(save.join(format!("{}.%(ext)s", vid)));
    cmd.args(["--playlist-items", "1", "--newline"]);
    cmd.args(cookie_args(&cookies));
    cmd.arg(&url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    bins::hide_console(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            send(Ev::Err { id, msg: format!("failed to start yt-dlp: {}", e) });
            return;
        }
    };
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    *handle.lock().unwrap() = Some(child);
    if is_cancelled() {
        // Cancel arrived before the child was registered.
        if let Some(c) = handle.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }

    // Drain stderr on its own thread (so neither pipe can fill and stall yt-dlp); keep the last line.
    let last_err = Arc::new(Mutex::new(String::new()));
    let le = Arc::clone(&last_err);
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            let s = String::from_utf8_lossy(&line).trim().to_string();
            if !s.is_empty() {
                *le.lock().unwrap() = s;
            }
        }
    });

    send(Ev::Progress { id, pct: 0.0 });
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        if let Some(pct) = pct_of(&String::from_utf8_lossy(&line)) {
            send(Ev::Progress { id, pct });
        }
    }
    let _ = err_thread.join();

    // stdout hit EOF, so the process is exiting: reap it.
    let status = handle.lock().unwrap().as_mut().map(|c| c.wait());
    match status {
        Some(Ok(st)) if st.success() => {
            let path = find_produced(&save, &vid).unwrap_or_else(|| save.clone());
            send(Ev::Done { id, path });
        }
        other => {
            let msg = if is_cancelled() {
                "Cancelled".to_string()
            } else {
                let detail = last_err.lock().unwrap().clone();
                match other {
                    Some(Ok(st)) => match st.code() {
                        Some(c) => format!("yt-dlp exited with {}: {}", c, detail),
                        None => format!("yt-dlp terminated: {}", detail),
                    },
                    Some(Err(e)) => format!("wait failed: {}", e),
                    None => "yt-dlp process missing".to_string(),
                }
            };
            send(Ev::Err { id, msg });
        }
    }
}

/// Extract the percentage from a line like "[download]  12.3% of ~50MiB at ...".
fn pct_of(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("[download]")?;
    let end = rest.find('%')?;
    rest[..end].trim().parse::<f64>().ok()
}

/// Run `yt-dlp -J --no-download <url>` and return the first entry's (title, id).
fn fetch_metadata(
    bin: &Path,
    url: &str,
    cookies: &Option<String>,
) -> Option<(String, String)> {
    let mut cmd = Command::new(bin);
    cmd.args(["--remote-components", "ejs:npm"]);
    cmd.args(cookie_args(cookies));
    cmd.args(["-J", "--no-download", "--playlist-items", "1"]).arg(url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    bins::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let obj = v
        .get("entries")
        .and_then(|e| e.as_array())
        .and_then(|e| e.first())
        .unwrap_or(&v);
    let title = obj.get("title").and_then(|t| t.as_str()).unwrap_or("Untitled").to_string();
    let id = obj.get("id").and_then(|t| t.as_str()).unwrap_or("").to_string();
    Some((title, id))
}

/// Derive a video-id token from the URL (fallback when metadata didn't give one).
fn derive_id(url: &str) -> String {
    let u = url.trim();
    for marker in ["?v=", "&v=", "youtu.be/", "/shorts/", "/embed/"] {
        if let Some(i) = u.find(marker) {
            let id: String = u[i + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(11)
                .collect();
            if !id.is_empty() {
                return id;
            }
        }
    }
    "video".to_string()
}

/// After success, find the file yt-dlp wrote (name starts with "<id>.").
fn find_produced(save: &Path, id: &str) -> Option<PathBuf> {
    let prefix = format!("{}.", id);
    std::fs::read_dir(save)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().map_or(false, |n| n.to_string_lossy().starts_with(&prefix))
        })
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

/// Run `<bin> --version` and return the version string, or None if missing/failed.
fn tool_version(bin: &Option<PathBuf>, is_ffmpeg: bool) -> Option<String> {
    let bin = bin.as_ref()?;
    let mut cmd = Command::new(bin);
    cmd.arg(if is_ffmpeg { "-version" } else { "--version" });
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    bins::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let v = if is_ffmpeg {
        // "ffmpeg version 7.1-essentials_build-www.gyan.dev Copyright ..."
        s.lines().next()?.split_whitespace().skip_while(|w| *w != "version").nth(1)?
    } else {
        s.split_whitespace().next()?
    };
    Some(v.to_string())
}

fn deno_version() -> Option<String> {
    let bin = bins::find_deno()?;
    let out = Command::new(bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
}