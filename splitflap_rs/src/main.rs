// Split-flap airport-style clock.
//
// Shows DAY / DATE (no year) / HOUR / MINUTE, each rendered as a row of
// mechanical "flap" tiles. Every tile only ever flips FORWARD through a
// fixed character sequence (never backward, never skipping) on a steady
// clock tick, exactly like a real Solari board - and every flip plays a
// short, procedurally generated mechanical "clack" so there's no dependency
// on any external/copyrighted audio file.

use std::f32::consts::PI;
use std::time::{Duration, Instant};

use chrono::Local;
use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, RichText, Rounding, Vec2};
use rand::Rng;

/// The physical order flaps are printed in on a real board. Columns only
/// ever step forward through this list, wrapping from the end to the start.
const FLAP_SEQUENCE: &[char] = &[
    ' ', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':',
];

/// Fixed time between flips, in milliseconds - a real board's motor runs at
/// a steady mechanical rate, not a random one.
const FLAP_INTERVAL_MS: u64 = 80;

/// Base filenames to look for, tried against every directory in
/// font_search_dirs() below.
const CUSTOM_FONT_FILENAMES: &[&str] = &["SplitFlapTV.ttf", "SplitFlapTV.otf"];
const CUSTOM_FONT_FAMILY: &str = "split_flap_tv";

/// Directories to check for the font, in priority order: a copy sitting
/// next to the binary (or in the current working directory) takes
/// precedence, then the usual OS font-install locations, so the app also
/// picks up a font that was simply installed via Font Book / a system
/// font manager rather than copied next to the executable.
fn font_search_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![std::path::PathBuf::from(".")];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join("Library/Fonts")); // macOS, user-installed (Font Book)
        dirs.push(home.join(".local/share/fonts")); // Linux, user-installed
        dirs.push(home.join(".fonts")); // Linux, older convention
    }

    dirs.push(std::path::PathBuf::from("/Library/Fonts")); // macOS, all-users
    dirs.push(std::path::PathBuf::from("/System/Library/Fonts")); // macOS, built-in
    dirs.push(std::path::PathBuf::from("/usr/share/fonts")); // Linux, system-wide
    dirs.push(std::path::PathBuf::from("/usr/local/share/fonts")); // Linux, system-wide

    dirs
}

fn flap_index(c: char) -> Option<usize> {
    FLAP_SEQUENCE.iter().position(|&f| f == c)
}

/// A single flap character cell.
struct FlapChar {
    current: usize,
    target: usize,
}

impl FlapChar {
    fn new(c: char) -> Self {
        let idx = flap_index(c).unwrap_or(0);
        Self {
            current: idx,
            target: idx,
        }
    }

    fn char(&self) -> char {
        FLAP_SEQUENCE[self.current]
    }

    fn set_target(&mut self, c: char) {
        if let Some(idx) = flap_index(c) {
            self.target = idx;
        }
    }

    /// Advances one flap forward if not already settled. Returns true if it moved.
    fn step(&mut self) -> bool {
        if self.current == self.target {
            return false;
        }
        self.current = (self.current + 1) % FLAP_SEQUENCE.len();
        true
    }
}

/// A labeled group of flap cells, e.g. "DAY" -> ['T','H','U'].
struct FlapGroup {
    label: &'static str,
    cells: Vec<FlapChar>,
}

impl FlapGroup {
    fn new(label: &'static str, initial: &str) -> Self {
        Self {
            label,
            cells: initial.chars().map(FlapChar::new).collect(),
        }
    }

    fn set_target(&mut self, s: &str) {
        for (cell, c) in self.cells.iter_mut().zip(s.chars()) {
            cell.set_target(c);
        }
    }

    /// Advances every unsettled cell one step. Returns true if anything moved.
    fn tick(&mut self) -> bool {
        let mut any = false;
        for cell in &mut self.cells {
            if cell.step() {
                any = true;
            }
        }
        any
    }

    #[allow(dead_code)]
    fn text(&self) -> String {
        self.cells.iter().map(FlapChar::char).collect()
    }
}

/// Generates a short, percussive mechanical "clack" - a fast noise
/// transient plus a low-frequency thump - entirely in code, so the app has
/// no dependency on any external sound asset.
fn generate_click_samples(sample_rate: u32) -> Vec<i16> {
    let duration_secs = 0.05_f32;
    let n = (sample_rate as f32 * duration_secs) as usize;
    let mut rng = rand::thread_rng();
    let mut samples = Vec::with_capacity(n);

    for i in 0..n {
        let t = i as f32 / sample_rate as f32;
        let noise: f32 = rng.gen_range(-1.0..1.0);
        let noise_env = (-t * 140.0).exp();
        let thump = (2.0 * PI * 90.0 * t).sin() * (-t * 35.0).exp();
        let sample = noise * noise_env * 0.5 + thump * 0.6;
        samples.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }

    samples
}

/// Thin wrapper so audio failures (e.g. no sound device) never crash the app.
struct AudioEngine {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
    click_samples: Vec<i16>,
    sample_rate: u32,
}

impl AudioEngine {
    fn try_new() -> Option<Self> {
        let (stream, handle) = rodio::OutputStream::try_default().ok()?;
        let sample_rate = 44_100;
        Some(Self {
            _stream: stream,
            handle,
            click_samples: generate_click_samples(sample_rate),
            sample_rate,
        })
    }

    fn play_click(&self) {
        let source =
            rodio::buffer::SamplesBuffer::new(1, self.sample_rate, self.click_samples.clone());
        use rodio::Source;
        let _ = self.handle.play_raw(source.convert_samples());
    }
}

struct SplitFlapApp {
    day: FlapGroup,
    date: FlapGroup,
    hour: FlapGroup,
    minute: FlapGroup,
    last_tick: Instant,
    audio: Option<AudioEngine>,
    font_loaded: bool,
}

impl SplitFlapApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let font_loaded = Self::load_custom_font(&cc.egui_ctx);
        if !font_loaded {
            eprintln!(
                "'Split-Flap TV' font not found (checked the working directory, next to the \
                 executable, and the usual system font folders e.g. ~/Library/Fonts on macOS); \
                 falling back to the built-in monospace font. Download it free at \
                 https://splitflaptv.com/split-flap-font/ and install it (e.g. via Font Book) \
                 or place SplitFlapTV.ttf/.otf alongside this binary."
            );
        }

        let now = Local::now();
        let mut app = Self {
            day: FlapGroup::new("", "   "),
            date: FlapGroup::new("", "      "),
            hour: FlapGroup::new("", "  "),
            minute: FlapGroup::new("", "  "),
            last_tick: Instant::now(),
            audio: AudioEngine::try_new(),
            font_loaded,
        };
        app.retarget(now);
        app
    }

    fn load_custom_font(ctx: &egui::Context) -> bool {
        for dir in font_search_dirs() {
            for filename in CUSTOM_FONT_FILENAMES {
                let path = dir.join(filename);
                if let Ok(bytes) = std::fs::read(&path) {
                    let mut fonts = FontDefinitions::default();
                    fonts
                        .font_data
                        .insert(CUSTOM_FONT_FAMILY.to_owned(), FontData::from_owned(bytes));
                    fonts
                        .families
                        .entry(FontFamily::Name(CUSTOM_FONT_FAMILY.into()))
                        .or_default()
                        .insert(0, CUSTOM_FONT_FAMILY.to_owned());
                    ctx.set_fonts(fonts);
                    println!("Loaded Split-Flap TV font from {}", path.display());
                    return true;
                }
            }
        }
        false
    }

    fn flap_font(&self, size: f32) -> egui::FontId {
        if self.font_loaded {
            egui::FontId::new(size, FontFamily::Name(CUSTOM_FONT_FAMILY.into()))
        } else {
            egui::FontId::monospace(size)
        }
    }

    /// Recomputes DAY/DATE/HOUR/MIN targets from the current local time.
    fn retarget(&mut self, now: chrono::DateTime<Local>) {
        self.day
            .set_target(&now.format("%a").to_string().to_uppercase());
        self.date
            .set_target(&now.format("%d %b").to_string().to_uppercase());
        self.hour.set_target(&now.format("%H").to_string());
        self.minute.set_target(&now.format("%M").to_string());
    }

    /// One synchronized clock tick: retarget if the time has moved on, then
    /// advance every unsettled cell one flap forward.
    fn tick(&mut self) {
        self.retarget(Local::now());

        let mut any_moved = false;
        any_moved |= self.day.tick();
        any_moved |= self.date.tick();
        any_moved |= self.hour.tick();
        any_moved |= self.minute.tick();

        if any_moved {
            if let Some(audio) = &self.audio {
                audio.play_click();
            }
        }
    }

    fn draw_tile(ui: &mut egui::Ui, ch: char, font: egui::FontId, tile_size: Vec2) {
        let (rect, _response) = ui.allocate_exact_size(tile_size, egui::Sense::hover());
        let painter = ui.painter_at(rect);

        // Tile body
        painter.rect_filled(rect, Rounding::same(4.0), Color32::from_rgb(20, 20, 22));

        // Subtle top/bottom shading to suggest the two physical flap halves
        let top_half = egui::Rect::from_min_max(rect.min, rect.center_bottom());
        let bottom_half = egui::Rect::from_min_max(rect.left_center(), rect.max);
        painter.rect_filled(top_half, Rounding::ZERO, Color32::from_rgb(30, 30, 33));
        painter.rect_filled(bottom_half, Rounding::ZERO, Color32::from_rgb(14, 14, 16));

        // Character
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            ch,
            font,
            Color32::from_rgb(235, 235, 235),
        );

        // The horizontal seam where a real flap folds in half
        painter.line_segment(
            [rect.left_center(), rect.right_center()],
            egui::Stroke::new(1.5_f32, Color32::from_rgb(5, 5, 5)),
        );
    }

    fn draw_group(ui: &mut egui::Ui, group: &FlapGroup, font: egui::FontId, tile_size: Vec2) {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(group.label)
                    .size(12.0)
                    .color(Color32::from_rgb(150, 150, 150))
                    .monospace(),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for cell in &group.cells {
                    Self::draw_tile(ui, cell.char(), font.clone(), tile_size);
                }
            });
        });
    }
}

impl eframe::App for SplitFlapApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let interval = Duration::from_millis(FLAP_INTERVAL_MS);
        let now = Instant::now();
        if now.duration_since(self.last_tick) >= interval {
            self.last_tick = now;
            self.tick();
        }
        ctx.request_repaint_after(interval);

        // Day/date tiles: unchanged size and font.
        let tile_size = Vec2::new(92.0, 140.0);
        let font = self.flap_font(100.0);

        // Hour/minute tiles: SAME WIDTH as day/date (so columns still line
        // up), but a taller tile and a bigger font so the time reads larger.
        let time_tile_size = Vec2::new(tile_size.x, 180.0);
        let time_font = self.flap_font(140.0);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK).inner_margin(24.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 20.0;
                        ui.add_space((ui.available_width() - 520.0).max(0.0) / 2.0);
                        Self::draw_group(ui, &self.day, font.clone(), tile_size);
                        Self::draw_group(ui, &self.date, font.clone(), tile_size);
                    });
                    ui.add_space(40.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 20.0;
                        ui.add_space((ui.available_width() - 520.0).max(0.0) / 2.0);
                        Self::draw_group(ui, &self.hour, time_font.clone(), time_tile_size);
                        ui.vertical(|ui| {
                            ui.label(""); // keeps the colon vertically aligned with the tiles
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(":")
                                    .size(140.0)
                                    .color(Color32::from_rgb(200, 200, 200)),
                            );
                        });
                        Self::draw_group(ui, &self.minute, time_font.clone(), time_tile_size);
                    });
                });
            });
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 480.0])
            .with_min_inner_size([1300.0, 440.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Split Flap Clock",
        native_options,
        Box::new(|cc| Box::new(SplitFlapApp::new(cc))),
    )
}
