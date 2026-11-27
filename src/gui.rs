//! Janela gráfica do WRecorder (egui) — visual "glass", minimalista.

use crate::capture::{self, Monitor, WindowInfo};
use crate::config::{AudioFormat, AudioSource, Config, NoiseReduction, ThemeName};
use crate::hotkeys::{self, Action, HotkeyService};
use crate::icons::{self, Icon};
use crate::prepare::{self, Backend, TargetSpec};
use crate::theme::{self, text, white, TitleAction, AMBER, GREEN, RED};
use crate::worker::{self, Msg, Phase, Request, Shared};
use eframe::egui::{self, vec2, RichText};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WIDTH: f32 = 400.0;
const HEIGHT: f32 = 600.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Video,
    Audio,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    Screen,
    Region,
    Window,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Main,
    Settings,
}

struct GuiApp {
    cfg: Config,
    backend: Backend,
    tx: Sender<Msg>,
    shared: Shared,
    view: View,
    mode: Mode,
    area: Area,
    monitors: Vec<Monitor>,
    /// Índice em `monitors`; `monitors.len()` = todas as telas.
    monitor_idx: usize,
    windows: Vec<WindowInfo>,
    window_idx: Option<usize>,
    last_phase: Phase,
    hotkey_status: Arc<Mutex<String>>,
    _hotkeys: Option<HotkeyService>,
    settings_msg: Option<(String, bool)>,
    startup_error: Option<String>,
    bg: egui::TextureHandle,
    /// Tema aplicado no momento (para detectar mudança).
    applied_theme: ThemeName,
}

pub fn run() -> anyhow::Result<()> {
    let (cfg, startup_error) = match Config::load_or_create() {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(format!("{e:#}"))),
    };
    let backend = Backend::detect(&cfg);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WRecorder")
            .with_app_id("wrecorder")
            .with_inner_size([WIDTH, HEIGHT])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_icon(Arc::new(window_icon())),
        ..Default::default()
    };
    eframe::run_native(
        "WRecorder",
        options,
        Box::new(move |cc| Ok(Box::new(GuiApp::new(cc, cfg, backend, startup_error)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, cfg: Config, backend: Backend, startup_error: Option<String>) -> GuiApp {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx, cfg.theme);
        let applied_theme = cfg.theme;
        let bg = theme::background_texture(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let (tx, shared) = worker::spawn(backend, move || ctx.request_repaint());

        // Atalhos globais -> thread do gravador.
        let (htx, hrx) = mpsc::channel::<Action>();
        let wtx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(a) = hrx.recv() {
                if wtx.send(Msg::Action(a)).is_err() {
                    break;
                }
            }
        });
        let hotkey_status = Arc::new(Mutex::new(String::from("registrando atalhos…")));
        let mut service = None;
        if backend.is_wayland() {
            // O portal pode mostrar um diálogo de confirmação: não bloqueia a janela.
            let (keys, status) = (cfg.hotkeys.clone(), hotkey_status.clone());
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let svc = HotkeyService::new(&keys, true, htx);
                *status.lock().unwrap() = svc.status.clone();
                ctx.request_repaint();
                std::thread::park(); // mantém a sessão do portal viva
                drop(svc);
            });
        } else {
            // X11/Windows: precisa ser criado na thread do laço de eventos da janela.
            let svc = HotkeyService::new(&cfg.hotkeys, false, htx);
            *hotkey_status.lock().unwrap() = svc.status.clone();
            service = Some(svc);
        }

        let monitors = if backend.is_wayland() { Vec::new() } else { capture::monitors().unwrap_or_default() };
        let monitor_idx = monitors.iter().position(|m| m.primary).unwrap_or(0);
        let mut app = GuiApp {
            view: View::Main,
            mode: Mode::Video,
            area: Area::Screen,
            monitors,
            monitor_idx,
            windows: Vec::new(),
            window_idx: None,
            last_phase: Phase::Idle,
            hotkey_status,
            _hotkeys: service,
            settings_msg: None,
            startup_error,
            cfg,
            backend,
            tx,
            shared,
            bg,
            applied_theme,
        };
        app.refresh_windows();
        app
    }

    fn refresh_windows(&mut self) {
        if self.backend.is_wayland() {
            return;
        }
        let prev = self.window_idx.and_then(|i| self.windows.get(i)).map(|w| w.id);
        self.windows = capture::windows().unwrap_or_default();
        self.window_idx = prev.and_then(|id| self.windows.iter().position(|w| w.id == id));
    }

    /// Monta o pedido de gravação a partir do que está selecionado.
    fn request(&self) -> Option<Request> {
        let target = match self.mode {
            Mode::Audio => None,
            Mode::Video => Some(match self.area {
                Area::Screen => TargetSpec::Screen(self.monitors.get(self.monitor_idx).cloned()),
                Area::Region => TargetSpec::SelectRegion(self.monitors.get(self.monitor_idx).cloned()),
                Area::Window => {
                    let w = self.window_idx.and_then(|i| self.windows.get(i)).cloned();
                    if w.is_none() && !self.backend.is_wayland() {
                        return None;
                    }
                    TargetSpec::Window(w)
                }
            }),
        };
        let audio = match self.mode {
            Mode::Video => self.cfg.video.audio,
            Mode::Audio => self.cfg.audio.source,
        };
        Some(Request { cfg: self.cfg.clone(), target, audio, format: self.cfg.audio.format })
    }

    fn send(&self, a: Action) {
        let _ = self.tx.send(Msg::Action(a));
    }

    /// Valida e salva a configuração. Retorna false se houver erro.
    fn save_settings(&mut self) -> bool {
        match hotkeys::validate(&self.cfg.hotkeys).and_then(|_| self.cfg.save()) {
            Ok(()) => {
                self.settings_msg = None;
                true
            }
            Err(e) => {
                self.settings_msg = Some((format!("{e:#}"), false));
                false
            }
        }
    }

    // ============================================================ tela principal

    fn draw_main(&mut self, ui: &mut egui::Ui) {
        let st = self.shared.lock().unwrap().clone();
        let idle = st.phase == Phase::Idle;

        if let Some(e) = &self.startup_error {
            theme::tinted_card(AMBER).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("Configuração inválida, usando padrões.\n{e}")).small().color(text(0)));
            });
        }

        ui.add_enabled_ui(idle, |ui| {
            theme::segmented(
                ui,
                "mode",
                &mut self.mode,
                &[(Mode::Video, Some(Icon::Monitor), "Tela"), (Mode::Audio, Some(Icon::Mic), "Áudio")],
                38.0,
            );
            ui.add_space(6.0);
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.draw_source(ui);
            });
        });

        ui.add_space(14.0);
        self.draw_controls(ui, &st);
        ui.add_space(10.0);
        self.draw_result(ui, &st);
    }

    fn draw_source(&mut self, ui: &mut egui::Ui) {
        match self.mode {
            Mode::Video => {
                theme::segmented(
                    ui,
                    "area",
                    &mut self.area,
                    &[(Area::Screen, None, "Tela inteira"), (Area::Region, None, "Área"), (Area::Window, None, "Janela")],
                    32.0,
                );
                ui.add_space(4.0);
                if !self.backend.is_wayland() {
                    match self.area {
                        Area::Screen | Area::Region => {
                            let allow_all = self.area == Area::Screen && self.monitors.len() > 1;
                            if self.monitor_idx >= self.monitors.len() && !allow_all {
                                self.monitor_idx = 0;
                            }
                            let name = |i: usize, ms: &[Monitor]| match ms.get(i) {
                                Some(m) => format!("{}  ·  {}×{}", m.name, m.rect.w, m.rect.h),
                                None => "Todas as telas".into(),
                            };
                            theme::row(ui, "Monitor", |ui| {
                                egui::ComboBox::from_id_salt("monitor")
                                    .width(ui.available_width())
                                    .selected_text(name(self.monitor_idx, &self.monitors))
                                    .show_ui(ui, |ui| {
                                        for i in 0..self.monitors.len() + usize::from(allow_all) {
                                            ui.selectable_value(&mut self.monitor_idx, i, name(i, &self.monitors));
                                        }
                                    });
                            });
                        }
                        Area::Window => {
                            theme::row(ui, "Janela", |ui| {
                                let text_ = self
                                    .window_idx
                                    .and_then(|i| self.windows.get(i))
                                    .map(|w| truncate(&w.title, 28))
                                    .unwrap_or_else(|| "Escolha…".into());
                                egui::ComboBox::from_id_salt("window")
                                    .width(ui.available_width() - 40.0)
                                    .selected_text(text_)
                                    .show_ui(ui, |ui| {
                                        for (i, w) in self.windows.iter().enumerate() {
                                            ui.selectable_value(&mut self.window_idx, Some(i), truncate(&w.label(), 60));
                                        }
                                    });
                                if theme::round_button(ui, Icon::Refresh, 30.0, "Atualizar lista").clicked() {
                                    self.refresh_windows();
                                }
                            });
                        }
                    }
                }
                theme::row(ui, "Áudio", |ui| audio_combo(ui, "vaudio", &mut self.cfg.video.audio, true));
                if self.cfg.video.audio.has_mic() {
                    theme::row(ui, "Ruído", |ui| noise_combo(ui, &mut self.cfg.audio.noise_reduction));
                }
                let hint = match (self.area, self.backend.is_wayland()) {
                    (Area::Region, _) => Some("Ao gravar, arraste para marcar a área · Esc cancela"),
                    (_, true) => Some("A tela ou janela é escolhida no diálogo do sistema"),
                    _ => None,
                };
                if let Some(h) = hint {
                    ui.label(RichText::new(h).size(11.5).color(text(2)));
                }
            }
            Mode::Audio => {
                theme::segmented(
                    ui,
                    "fmt",
                    &mut self.cfg.audio.format,
                    &[(AudioFormat::Mp3, None, "MP3"), (AudioFormat::Wav, None, "WAV · sem perdas")],
                    32.0,
                );
                ui.add_space(4.0);
                theme::row(ui, "Fonte", |ui| audio_combo(ui, "aaudio", &mut self.cfg.audio.source, false));
                if self.cfg.audio.source.has_mic() {
                    theme::row(ui, "Ruído", |ui| noise_combo(ui, &mut self.cfg.audio.noise_reduction));
                }
            }
        }
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui, st: &worker::Status) {
        let (label, color, pulse) = match st.phase {
            Phase::Idle => ("Pronto", text(2), false),
            Phase::Preparing => ("Escolha o que gravar", AMBER, false),
            Phase::Countdown => ("Começando", AMBER, false),
            Phase::Recording => ("Gravando", RED, true),
            Phase::Paused => ("Pausado", AMBER, false),
            Phase::Finishing => ("Salvando", AMBER, false),
        };
        theme::status_pill(ui, label, color, pulse);
        ui.add_space(2.0);
        let active = matches!(st.phase, Phase::Recording | Phase::Paused);
        theme::timer(ui, &fmt_dur(st.elapsed), 54.0, if active { text(0) } else { white(150) });
        ui.add_space(8.0);

        // Botões: [pausar]  (gravar/parar)  [espaço]
        let req = self.request();
        if st.phase == Phase::Idle {
            self.shared.lock().unwrap().request = req.clone();
        }
        let side = 48.0;
        let gap = 26.0;
        let total = 78.0 + 2.0 * (side + gap);
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - total) / 2.0);
            ui.spacing_mut().item_spacing.x = gap;
            // esquerda: pausar / retomar
            ui.allocate_ui(vec2(side, 78.0), |ui| {
                ui.centered_and_justified(|ui| {
                    if active {
                        let (icon, tip) = if st.phase == Phase::Recording { (Icon::Pause, "Pausar") } else { (Icon::Play, "Retomar") };
                        if theme::round_button(ui, icon, side, tip).clicked() {
                            self.send(Action::Pause);
                        }
                    } else {
                        ui.allocate_space(vec2(side, side));
                    }
                });
            });
            // centro: gravar / parar
            let busy = matches!(st.phase, Phase::Preparing | Phase::Countdown | Phase::Finishing);
            let ok = req.is_some() || active;
            let resp = ui.add_enabled_ui(ok, |ui| theme::record_button(ui, active, busy)).inner;
            let resp = resp.on_hover_text(if active { "Parar e salvar" } else { "Gravar" });
            if resp.clicked() && !busy {
                self.send(if active { Action::Stop } else { Action::Start });
            }
            ui.allocate_space(vec2(side, side));
        });

        ui.add_space(8.0);
        let k = &self.cfg.hotkeys;
        let hint = if st.phase == Phase::Idle && req.is_none() {
            RichText::new("Escolha uma janela na lista").color(AMBER)
        } else {
            RichText::new(format!("{}  gravar   ·   {}  pausar   ·   {}  parar", k.start, k.pause, k.stop)).color(text(2))
        };
        ui.vertical_centered(|ui| {
            let status = self.hotkey_status.lock().unwrap().clone();
            let r = ui.label(hint.size(11.0));
            if status.contains("indispon") {
                ui.label(RichText::new("atalhos globais indisponíveis · use os botões").size(11.0).color(AMBER))
                    .on_hover_text(status);
            } else {
                r.on_hover_text(status);
            }
        });
    }

    fn draw_result(&mut self, ui: &mut egui::Ui, st: &worker::Status) {
        if let Some(err) = &st.error {
            let mut dismiss = false;
            theme::tinted_card(RED).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                    icons::paint(ui.painter(), r, Icon::Warning, RED);
                    ui.add(egui::Label::new(RichText::new(truncate(err, 400)).size(12.0).color(text(0))).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if icons::small(ui, Icon::Close, "Dispensar").clicked() {
                            dismiss = true;
                        }
                    });
                });
            });
            if dismiss {
                self.shared.lock().unwrap().error = None;
            }
            ui.add_space(6.0);
        }
        if let Some(path) = &st.last_file {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) as f64 / 1_048_576.0;
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 15.0, GREEN.gamma_multiply(0.22));
                    icons::paint(ui.painter(), r.shrink(8.0), Icon::Check, GREEN);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(RichText::new(truncate(&name, 30)).size(13.0).family(theme::semibold()).color(text(0)));
                        ui.label(RichText::new(format!("{size:.1} MB · salvo")).size(11.5).color(text(2)));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::round_button(ui, Icon::Folder, 34.0, "Mostrar na pasta").clicked() {
                            prepare::reveal(path);
                        }
                        if theme::round_button(ui, Icon::Play, 34.0, "Abrir").clicked() {
                            prepare::open_file(path);
                        }
                    });
                });
            });
        }
    }

    // ============================================================ configurações

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                let c = &mut self.cfg;

                theme::section(ui, "Aparência");
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    theme::theme_picker(ui, &mut c.theme);
                });

                ui.add_space(10.0);
                theme::section(ui, "Vídeo");
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Quadros por segundo").color(text(1)).size(12.5));
                    theme::segmented(ui, "fps", &mut c.video.fps, &[(24, None, "24"), (30, None, "30"), (60, None, "60")], 30.0);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Qualidade").color(text(1)).size(12.5));
                    let mut q = match c.video.quality {
                        0..=19 => 18,
                        20..=25 => 23,
                        _ => 28,
                    };
                    if theme::segmented(ui, "quality", &mut q, &[(28, None, "Econômica"), (23, None, "Normal"), (18, None, "Alta")], 30.0).changed() {
                        c.video.quality = q;
                    }
                    ui.add_space(4.0);
                    theme::row(ui, "Encoder", |ui| {
                        egui::ComboBox::from_id_salt("enc").width(ui.available_width()).selected_text(encoder_label(&c.video.encoder)).show_ui(ui, |ui| {
                            for e in ["auto", "libx264", "h264_nvenc", "h264_qsv", "h264_amf"] {
                                ui.selectable_value(&mut c.video.encoder, e.to_string(), encoder_label(e));
                            }
                        });
                    });
                    theme::toggle_row(ui, "Mostrar cursor", &mut c.video.show_cursor);
                    theme::toggle_row(ui, "Minimizar ao gravar", &mut c.minimize_on_record);
                });

                ui.add_space(10.0);
                theme::section(ui, "Áudio");
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Redução de ruído do microfone").color(text(1)).size(12.5));
                    theme::segmented(
                        ui,
                        "nr",
                        &mut c.audio.noise_reduction,
                        &[(NoiseReduction::Off, None, "Desligada"), (NoiseReduction::Light, None, "Leve"), (NoiseReduction::Strong, None, "Forte")],
                        30.0,
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new("Qualidade do MP3").color(text(1)).size(12.5));
                    theme::segmented(
                        ui,
                        "mp3",
                        &mut c.audio.mp3_bitrate,
                        &[(128, None, "128k"), (192, None, "192k"), (256, None, "256k"), (320, None, "320k")],
                        30.0,
                    );
                });

                ui.add_space(10.0);
                theme::section(ui, "Arquivos");
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let hint = c.output_dir(true).display().to_string();
                        ui.add(
                            egui::TextEdit::singleline(&mut c.output_dir)
                                .hint_text(hint)
                                .desired_width(ui.available_width() - 42.0)
                                .margin(vec2(10.0, 7.0)),
                        );
                        if theme::round_button(ui, Icon::Folder, 32.0, "Abrir pasta").clicked() {
                            let dir = c.output_dir(self.mode == Mode::Video);
                            let _ = std::fs::create_dir_all(&dir);
                            prepare::reveal(&dir);
                        }
                    });
                    ui.label(RichText::new("Vazio = pastas Vídeos / Música").size(11.0).color(text(2)));
                });

                ui.add_space(10.0);
                theme::section(ui, "Atalhos");
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (label, v) in [("Gravar", &mut c.hotkeys.start), ("Pausar", &mut c.hotkeys.pause), ("Parar", &mut c.hotkeys.stop)] {
                        theme::row(ui, label, |ui| {
                            ui.add(egui::TextEdit::singleline(v).desired_width(ui.available_width()).margin(vec2(10.0, 6.0)));
                        });
                    }
                    let note = if self.backend.is_wayland() {
                        "Valem ao reabrir o app. No Wayland também dá para mudar em Configurações › Teclado."
                    } else {
                        "Novos atalhos valem ao reabrir o app."
                    };
                    ui.label(RichText::new(note).size(11.0).color(text(2)));
                });

                ui.add_space(12.0);
                if let Some((msg, ok)) = &self.settings_msg {
                    ui.label(RichText::new(msg).size(12.0).color(if *ok { GREEN } else { RED }));
                }
                ui.horizontal(|ui| {
                    if theme::pill_button(ui, "Restaurar padrões", false).clicked() {
                        self.cfg = Config::default();
                        self.settings_msg = Some(("Padrões restaurados — clique em Salvar.".into(), true));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::pill_button(ui, "Salvar", true).clicked() && self.save_settings() {
                            self.view = View::Main;
                        }
                    });
                });
                ui.add_space(8.0);
            });
    }
}

impl eframe::App for GuiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4] // janela transparente: os cantos arredondados são pintados por nós
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let phase = self.shared.lock().unwrap().phase;
        if phase != self.last_phase {
            match phase {
                Phase::Countdown => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                Phase::Idle if self.last_phase == Phase::Finishing => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                _ => {}
            }
            self.last_phase = phase;
        }
        if matches!(phase, Phase::Recording | Phase::Paused) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.cfg.theme != self.applied_theme {
            theme::apply_style(ui.ctx(), self.cfg.theme);
            self.bg = theme::background_texture(ui.ctx());
            self.applied_theme = self.cfg.theme;
        }
        let full = ui.max_rect();
        theme::paint_window(ui, full, &self.bg);

        let (title, icon, tip) = match self.view {
            View::Main => ("WRecorder", Icon::Gear, "Configurações"),
            View::Settings => ("Configurações", Icon::Back, "Voltar"),
        };
        let bar = egui::Rect::from_min_size(full.min, vec2(full.width(), 44.0));
        match theme::title_bar(ui, bar, title, icon, tip) {
            TitleAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            TitleAction::Minimize => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            TitleAction::Right => {
                self.view = match self.view {
                    View::Main => {
                        self.settings_msg = None;
                        View::Settings
                    }
                    View::Settings => {
                        if self.save_settings() {
                            View::Main
                        } else {
                            View::Settings
                        }
                    }
                }
            }
            TitleAction::None => {}
        }

        let content = egui::Rect::from_min_max(full.min + vec2(18.0, 48.0), full.max - vec2(18.0, 14.0));
        let idle = self.shared.lock().unwrap().phase == Phase::Idle;
        if !idle && self.view == View::Settings {
            self.view = View::Main;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| match self.view {
            View::Main => self.draw_main(ui),
            View::Settings => self.draw_settings(ui),
        });
    }
}

impl Drop for GuiApp {
    fn drop(&mut self) {
        // Lembra as últimas escolhas (fonte de áudio, formato, ruído...).
        if hotkeys::validate(&self.cfg.hotkeys).is_ok() {
            let _ = self.cfg.save();
        }
        // Fechar a janela no meio da gravação salva o que já foi gravado.
        self.send(Action::Stop);
        let start = std::time::Instant::now();
        while self.shared.lock().map(|s| s.phase != Phase::Idle).unwrap_or(false) && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn encoder_label(e: &str) -> String {
    match e {
        "auto" => "Automático (GPU se houver)".into(),
        "libx264" => "x264 (CPU)".into(),
        "h264_nvenc" => "NVIDIA NVENC".into(),
        "h264_qsv" => "Intel Quick Sync".into(),
        "h264_amf" => "AMD AMF".into(),
        other => other.into(),
    }
}

fn audio_combo(ui: &mut egui::Ui, id: &str, value: &mut AudioSource, allow_none: bool) {
    egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(value.label()).show_ui(ui, |ui| {
        let opts: &[AudioSource] = if allow_none {
            &[AudioSource::None, AudioSource::Mic, AudioSource::System, AudioSource::Both]
        } else {
            &[AudioSource::Mic, AudioSource::System, AudioSource::Both]
        };
        for o in opts {
            ui.selectable_value(value, *o, o.label());
        }
    });
}

fn noise_combo(ui: &mut egui::Ui, value: &mut NoiseReduction) {
    egui::ComboBox::from_id_salt("noise")
        .width(ui.available_width())
        .selected_text(value.label())
        .show_ui(ui, |ui| {
            for o in [NoiseReduction::Off, NoiseReduction::Light, NoiseReduction::Strong] {
                ui.selectable_value(value, o, o.label());
            }
        })
        .response
        .on_hover_text("Remove chiado e ruído de fundo do microfone");
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Ícone da janela: círculo vermelho sobre quadrado escuro arredondado.
pub fn window_icon() -> egui::IconData {
    let n = 64u32;
    let mut rgba = vec![0u8; (n * n * 4) as usize];
    let c = (n as f32 - 1.0) / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 - c, y as f32 - c);
            let r = 14.0;
            let qx = (fx.abs() - (c - r)).max(0.0);
            let qy = (fy.abs() - (c - r)).max(0.0);
            let in_bg = (qx * qx + qy * qy).sqrt() <= r;
            let d = (fx * fx + fy * fy).sqrt();
            let i = ((y * n + x) * 4) as usize;
            let px: [u8; 4] = if d <= 17.0 {
                [255, 69, 58, 255]
            } else if d <= 21.0 {
                [240, 240, 240, 255]
            } else if in_bg {
                [32, 33, 44, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    egui::IconData { rgba, width: n, height: n }
}
