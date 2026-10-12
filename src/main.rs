#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bins;
mod engine;

use eframe::egui;
use engine::{Cmd, Ev, Job, JobStatus, Quality};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::Duration;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x17, 0x4D); // YouTube-ish red
const GOOD: egui::Color32 = egui::Color32::from_rgb(0x7C, 0xFF, 0x8A);
const BAD: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x6B, 0x6B);

fn main() -> eframe::Result {
    let save = bins::default_save_dir();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TubeGrab")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "TubeGrab",
        opts,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut style = (*cc.egui_ctx.style()).clone();
            style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(28, 28, 30);
            style.visuals.selection.bg_fill = ACCENT;
            cc.egui_ctx.set_style(style);
            Ok(Box::new(App::new(save)))
        }),
    )
}

enum Action {
    Cancel(usize),
    OpenFolder(PathBuf),
}

struct App {
    save_dir: PathBuf,
    quality: Quality,
    input: String,
    jobs: Vec<Job>,
    next_id: usize,
    worker_tx: Sender<Cmd>,
    ev_rx: Receiver<Ev>,
    ytdlp_status: Option<String>,
    ffmpeg_status: Option<String>,
    deno_status: Option<String>,
    installing: bool,
    pending_folder: Option<Receiver<PathBuf>>,
    log: VecDeque<String>,
    cookies: Option<&'static str>,
}

impl App {
    fn new(save_dir: PathBuf) -> Self {
        let save_dir = if save_dir.exists() { save_dir } else { bins::default_save_dir() };
        let (worker_tx, ev_rx) = engine::spawn_worker(bins::find_ytdlp(), bins::find_ffmpeg());
        let _ = worker_tx.send(Cmd::CheckTools);
        App {
            save_dir,
            quality: Quality::Best,
            input: String::new(),
            jobs: Vec::new(),
            next_id: 0,
            worker_tx,
            ev_rx,
            ytdlp_status: None,
            ffmpeg_status: None,
            deno_status: None,
            installing: false,
            pending_folder: None,
            log: VecDeque::new(),
            cookies: None,
        }
    }

    fn apply_ev(&mut self, ev: Ev) {
        match ev {
            Ev::Meta { id, title } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.title = title;
                    if matches!(j.status, JobStatus::Queued) {
                        j.status = JobStatus::FetchingMeta;
                    }
                }
            }
            Ev::Progress { id, pct } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.status = JobStatus::Downloading { pct };
                }
            }
            Ev::Done { id, path } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.status = JobStatus::Done(path.clone());
                }
                self.push_log(format!("#{} done → {}", id, path.display()));
            }
            Ev::Err { id, msg } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.status = JobStatus::Error(msg.clone());
                }
                self.push_log(format!("#{}: {}", id, msg));
            }
            Ev::YtdlpStatus(s) => {
                self.ytdlp_status = s;
                self.installing = false;
            }
            Ev::FfmpegStatus(s) => {
                self.ffmpeg_status = s;
                self.installing = false;
            }
            Ev::DenoStatus(s) => {
                self.deno_status = s;
                self.installing = false;
            }
            Ev::Log(l) => self.push_log(l),
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > 200 {
            self.log.pop_front();
        }
    }

    fn poll_folder_picker(&mut self) {
        let Some(rx) = &self.pending_folder else { return };
        match rx.try_recv() {
            Ok(p) => {
                self.save_dir = p;
                self.pending_folder = None;
            }
            Err(TryRecvError::Disconnected) => self.pending_folder = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui) {
        let (ys, fs, ds, installing) = (self.ytdlp_status.clone(), self.ffmpeg_status.clone(), self.deno_status.clone(), self.installing,);
        let mut installs: Vec<Cmd> = Vec::new();
        ui.horizontal(|ui| {
            ui.heading("TubeGrab");
            // right_to_left: first item added ends up rightmost
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                installs.extend(tool_chip(ui, "ffmpeg", &fs, installing, Cmd::InstallFfmpeg));
                ui.separator();
                installs.extend(tool_chip(ui, "yt-dlp", &ys, installing, Cmd::InstallYtdlp));
                ui.separator();
                installs.extend(tool_chip(ui, "Deno",   &ds, installing, if ds.is_some() {Cmd::UpdateDeno} else {Cmd::InstallDeno},));
                ui.separator();
            });
        });
        if let Some(cmd) = installs.into_iter().next() {
            self.installing = true;
            let _ = self.worker_tx.send(cmd); // cleared when the matching status event arrives
        }
    }

    fn draw_input(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let folder_name = self
            .save_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.save_dir.display().to_string());
        let full_path = self.save_dir.display().to_string();

        ui.horizontal(|ui| {
            ui.label("Quality");
            egui::ComboBox::from_id_salt("quality")
                .selected_text(self.quality.label())
                .show_ui(ui, |ui| {
                    for q in [Quality::Best, Quality::P1080, Quality::P720, Quality::Mp3] {
                        ui.selectable_value(&mut self.quality, q, q.label());
                    }
                });
            ui.separator();
            ui.label("Cookies");
            egui::ComboBox::from_id_salt("cookies")
                .selected_text(self.cookies.unwrap_or("none"))
                .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.cookies, None, "none");
                    for b in ["firefox", "chrome", "edge", "brave"] {
                        ui.selectable_value(&mut self.cookies, Some(b), b);
                    }
         });
            ui.separator();
            ui.label("Save to");
            if ui.button(format!("{}…", folder_name)).on_hover_text(full_path).clicked() {
                self.pick_folder(ctx);
            }
        });
        ui.add_space(6.0);

        let width = (ui.available_width() - 170.0).max(200.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label("Paste YouTube links — one per line:");
                ui.add(
                    egui::TextEdit::multiline(&mut self.input)
                        .desired_rows(5)
                        .desired_width(width)
                        .hint_text("https://www.youtube.com/watch?v=…  (or bare IDs, youtu.be links)"),
                );
            });
            ui.separator();
            ui.vertical(|ui| {
                if ui.add(egui::Button::new("Add to queue").fill(ACCENT)).clicked() {
                    self.add_urls();
                }
                ui.add_space(4.0);
                if ui.small_button("Clear finished").clicked() {
                    self.jobs
                        .retain(|j| !matches!(j.status, JobStatus::Done(_) | JobStatus::Error(_)));
                }
                if self.jobs.iter().any(|j| matches!(j.status, JobStatus::Done(_))) {
                    if ui.small_button("Open folder").clicked() {
                        bins::open_folder(&self.save_dir);
                    }
                }
            });
        });
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        if self.pending_folder.is_some() {
            return; // a dialog is already open
        }
        let start = self.save_dir.clone();
        let (tx, rx) = channel::<PathBuf>();
        std::thread::spawn(move || {
            let _ = tx.send(bins::pick_folder(start));
        });
        self.pending_folder = Some(rx);
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn draw_queue(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(format!("Queue ({})", self.jobs.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Open downloads").clicked() {
                    bins::open_folder(&self.save_dir);
                }
            });
        });
        ui.add_space(4.0);

        let mut actions: Vec<Action> = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("queue")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.jobs.is_empty() {
                    ui.add_space(24.0);
                    ui.label(
                        egui::RichText::new("Nothing queued. Paste links above and hit “Add to queue”.").weak(),
                    );
                    return;
                }
                for j in &self.jobs {
                    job_row(ui, j, &self.save_dir, &mut actions);
                    ui.separator();
                }
            });

        for a in actions {
            match a {
                Action::Cancel(id) => {
                    let _ = self.worker_tx.send(Cmd::Cancel(id));
                }
                Action::OpenFolder(p) => bins::open_folder(&p),
            }
        }
    }

    fn draw_console(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("console")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for l in self.log.iter() {
                    ui.label(egui::RichText::new(l).monospace().weak());
                }
            });
    }

    fn add_urls(&mut self) {
        let urls: Vec<String> = self
            .input
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(normalize_url)
            .collect();
        if urls.is_empty() {
            return;
        }
        for url in urls {
            let id = self.next_id;
            self.next_id += 1;
            self.jobs.push(Job { id, url: url.clone(), title: url.clone(), status: JobStatus::Queued });
            let _ = self.worker_tx.send(Cmd::Enqueue {
                id,
                url: url.clone(),
                quality: self.quality,
                save_dir: self.save_dir.clone(),
                cookies: self.cookies.map(String::from),
            });
            self.push_log(format!("#{} queued: {}", id, url));
        }
        self.input.clear();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain engine events that arrived since last frame (non-blocking).
        while let Ok(ev) = self.ev_rx.try_recv() {
            self.apply_ev(ev);
        }
        self.poll_folder_picker();

        // Tell the engine to stop its children when the window closes.
        if ctx.input(|i| i.viewport().close_requested()) {
            let _ = self.worker_tx.send(Cmd::Quit);
        }

        // The worker can't wake the UI, so poll a few times a second.
        ctx.request_repaint_after(Duration::from_millis(250));

        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(130.0)
            .show(ctx, |ui| self.draw_console(ui));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            self.draw_top_bar(ui);
            ui.add_space(8.0);
            self.draw_input(ui, ctx);
            ui.add_space(10.0);
            self.draw_queue(ui);
        });
    }
}

/// "name: version" label plus an Install/Update button. Returns the command if clicked.
fn tool_chip(ui: &mut egui::Ui, name: &str, status: &Option<String>, installing: bool, cmd: Cmd) -> Option<Cmd> {
    let ok = status.is_some();
    ui.label(
        egui::RichText::new(format!("{}: {}", name, status.as_deref().unwrap_or("not found")))
            .color(if ok { GOOD } else { BAD }),
    );
    let label = if ok {
        "Update"
    } else if installing {
        "Working…"
    } else {
        "Install"
    };
    let clicked = ui.add_enabled(!installing, egui::Button::new(label).small()).clicked();
    clicked.then_some(cmd)
}

fn job_row(ui: &mut egui::Ui, job: &Job, save_dir: &PathBuf, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let mut title = egui::RichText::new(&job.title);
        match job.status {
            JobStatus::Done(_) => title = title.strong(),
            JobStatus::Error(_) => title = title.weak(),
            _ => {}
        }
        ui.label(title).on_hover_text(&job.url);
        ui.add_space(10.0);

        match &job.status {
            JobStatus::Queued => {
                ui.label(egui::RichText::new("queued").weak());
            }
            JobStatus::FetchingMeta => {
                ui.add(egui::Spinner::new());
                ui.label(egui::RichText::new("resolving…").weak());
            }
            JobStatus::Downloading { pct } => {
                ui.add(
                    egui::ProgressBar::new((*pct as f32 / 100.0).clamp(0.0, 1.0))
                        .desired_width(200.0)
                        .text(format!("{:.0}%", pct)),
                );
            }
            JobStatus::Done(path) => {
                ui.label(egui::RichText::new("✓").color(GOOD));
                if ui.small_button("Open").clicked() {
                    let dir = if path.is_dir() {
                        path.clone()
                    } else {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| save_dir.clone())
                    };
                    actions.push(Action::OpenFolder(dir));
                }
            }
            JobStatus::Error(msg) => {
                ui.label(egui::RichText::new("✕").color(BAD));
                ui.label(egui::RichText::new(msg).color(BAD));
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if matches!(job.status, JobStatus::Downloading { .. } | JobStatus::FetchingMeta)
                && ui.small_button("Cancel").clicked()
            {
                actions.push(Action::Cancel(job.id));
            }
        });
    });
}

/// Normalize a pasted token into a full YouTube URL (handles bare IDs, youtu.be, scheme-less links).
fn normalize_url(s: &str) -> String {
    let t = s.trim();
    let bare = t.len() == 11 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if bare {
        return format!("https://www.youtube.com/watch?v={}", t);
    }
    if t.starts_with("http") {
        return t.to_string();
    }
    if let Some(rest) = t.strip_prefix("youtu.be/") {
        let id: String = rest.chars().take(11).collect();
        return format!("https://www.youtube.com/watch?v={}", id);
    }
    if t.contains('.') {
        return format!("https://{}", t);
    }
    t.to_string()
}

