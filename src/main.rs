//! WRecorder — gravador de tela (MP4) e áudio (WAV/MP3) para Linux e Windows.
//!
//! Sem argumentos abre a janela gráfica. Subcomandos oferecem o modo terminal.

// No Windows (release) não abre um console junto com a janela.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod capture;
mod config;
mod ffmpeg;
mod gui;
mod hotkeys;
mod icons;
mod theme;
mod install;
mod prepare;
mod recorder;
mod selector;
#[cfg(target_os = "linux")]
mod wayland;
mod worker;

use anyhow::{anyhow, bail, Context, Result};
use capture::Rect;
use clap::{Parser, Subcommand};
use config::{AudioFormat, AudioSource, Config, NoiseReduction};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Stylize;
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Select};
use hotkeys::{Action, HotkeyService};
use prepare::{Backend, TargetSpec};
use recorder::{Plan, Recorder};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "wrecorder", version, about = "Gravador de tela (MP4) e áudio (WAV/MP3). Sem argumentos abre a janela gráfica.")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Menu interativo no terminal.
    Menu,
    /// Grava a tela em MP4 (modo terminal).
    Video {
        /// Seleciona uma área. Sem valor abre o seletor; ou informe X,Y,LARGURA,ALTURA.
        #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["window", "monitor", "all_screens"])]
        region: Option<String>,
        /// Grava uma janela. Sem valor mostra a lista; ou parte do título (ex.: "Firefox").
        #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["monitor", "all_screens"])]
        window: Option<String>,
        /// Número do monitor (veja `wrecorder list`).
        #[arg(long)]
        monitor: Option<usize>,
        /// Todas as telas juntas (X11 / Windows).
        #[arg(long)]
        all_screens: bool,
        #[arg(long, value_enum)]
        audio: Option<AudioSource>,
        /// Redução de ruído do microfone.
        #[arg(long, value_enum)]
        denoise: Option<NoiseReduction>,
        #[arg(long)]
        fps: Option<u32>,
        /// Encoder do ffmpeg (auto, libx264, h264_nvenc, h264_qsv, h264_amf...).
        #[arg(long)]
        encoder: Option<String>,
        /// Arquivo ou pasta de saída.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Começa a gravar imediatamente (sem esperar o atalho).
        #[arg(long)]
        now: bool,
    },
    /// Grava somente áudio em WAV ou MP3 (modo terminal).
    Audio {
        #[arg(long, value_enum)]
        format: Option<AudioFormat>,
        #[arg(long, value_enum)]
        source: Option<AudioSource>,
        /// Redução de ruído do microfone.
        #[arg(long, value_enum)]
        denoise: Option<NoiseReduction>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        now: bool,
    },
    /// Lista monitores e janelas disponíveis.
    List,
    /// Mostra o caminho do arquivo de configuração (cria se não existir).
    Config {
        #[arg(long)]
        reset: bool,
    },
    /// Adiciona o WRecorder ao menu de aplicativos (busca do sistema / Menu Iniciar).
    Install,
    /// Remove o WRecorder do menu de aplicativos.
    Uninstall,
    /// Testa a captura de tela via PipeWire (Linux/Wayland) e mostra os erros.
    Diagnose,
    #[command(name = "__select", hide = true)]
    Select { file: PathBuf, at: Option<String> },
}

fn main() {
    #[cfg(windows)]
    {
        capture::win::set_dpi_aware();
        if std::env::args().len() > 1 {
            attach_console();
        }
    }
    if let Err(e) = run() {
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!("{} {e:#}", "erro:".red().bold());
        std::process::exit(1);
    }
}

/// Com `windows_subsystem = "windows"`, reconecta ao console de quem chamou (modo terminal).
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cmd = match cli.cmd {
        None => return gui::run(),
        Some(c) => c,
    };
    match cmd {
        Cmd::Select { file, at } => {
            let pos = at.as_deref().and_then(|s| {
                let (x, y) = s.split_once(',')?;
                Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
            });
            selector::run_selector(&file, pos)
        }
        Cmd::Config { reset } => {
            if reset {
                Config::default().save()?;
            } else {
                Config::load_or_create()?;
            }
            println!("{}", Config::path().display());
            Ok(())
        }
        Cmd::Install => {
            let p = install::install()?;
            println!("{} WRecorder instalado ({}). Procure por \"WRecorder\" no menu de aplicativos.", "✔".green(), p.display());
            Ok(())
        }
        Cmd::Uninstall => {
            install::uninstall()?;
            println!("WRecorder removido do menu de aplicativos.");
            Ok(())
        }
        Cmd::Diagnose => {
            #[cfg(target_os = "linux")]
            return wayland::diagnose(&Config::load_or_create()?.gst_launch);
            #[allow(unreachable_code)]
            Ok(())
        }
        other => terminal(other),
    }
}

// ====================================================================== modo terminal

struct App {
    cfg: Config,
    backend: Backend,
    hotkeys: Option<(HotkeyService, Receiver<Action>)>,
}

fn terminal(cmd: Cmd) -> Result<()> {
    let cfg = Config::load_or_create()?;
    hotkeys::validate(&cfg.hotkeys).with_context(|| format!("verifique {}", Config::path().display()))?;
    let mut app = App { backend: Backend::detect(&cfg), cfg, hotkeys: None };

    match cmd {
        Cmd::Menu => interactive(&mut app),
        Cmd::List => list(&app),
        Cmd::Video { region, window, monitor, all_screens, audio, denoise, fps, encoder, output, now } => {
            ffmpeg::check_available(&app.cfg.ffmpeg)?;
            if let Some(d) = denoise {
                app.cfg.audio.noise_reduction = d;
            }
            if let Some(f) = fps {
                app.cfg.video.fps = f.clamp(1, 240);
            }
            if let Some(e) = encoder {
                app.cfg.video.encoder = e;
            }
            let spec = if let Some(r) = region {
                if r.is_empty() {
                    match choose_monitor(&app, false)? {
                        Some(m) => TargetSpec::SelectRegion(m),
                        None => return Ok(()),
                    }
                } else {
                    TargetSpec::Region(Rect::parse(&r).ok_or_else(|| anyhow!("área inválida, use X,Y,LARGURA,ALTURA"))?)
                }
            } else if let Some(w) = window {
                match pick_window(&app, (!w.is_empty()).then_some(w.as_str()))? {
                    Some(s) => s,
                    None => return Ok(()),
                }
            } else if all_screens {
                TargetSpec::Screen(None)
            } else if let Some(n) = monitor {
                let mons = capture::monitors()?;
                TargetSpec::Screen(Some(mons.get(n).cloned().ok_or_else(|| anyhow!("monitor {n} não existe (veja `wrecorder list`)"))?))
            } else {
                let mons = if app.backend.is_wayland() { Vec::new() } else { capture::monitors().unwrap_or_default() };
                TargetSpec::Screen(mons.iter().find(|m| m.primary).or(mons.first()).cloned())
            };
            let audio = audio.unwrap_or(app.cfg.video.audio);
            run_video(&mut app, spec, audio, output, now)
        }
        Cmd::Audio { format, source, denoise, output, now } => {
            ffmpeg::check_available(&app.cfg.ffmpeg)?;
            if let Some(d) = denoise {
                app.cfg.audio.noise_reduction = d;
            }
            let format = format.unwrap_or(app.cfg.audio.format);
            let source = source.unwrap_or(app.cfg.audio.source);
            if source == AudioSource::None {
                bail!("escolha uma fonte de áudio (mic, system ou both)");
            }
            let plan = prepare::audio_plan(&app.cfg, format, source, output);
            record(&mut app, plan, now).map(|_| ())
        }
        _ => unreachable!(),
    }
}

fn run_video(app: &mut App, spec: TargetSpec, audio: AudioSource, output: Option<PathBuf>, now: bool) -> Result<()> {
    if app.backend.is_wayland() {
        println!("{}", "Escolha a tela/janela no diálogo do sistema…".dim());
    }
    if matches!(spec, TargetSpec::SelectRegion(_)) {
        println!("{}", "Arraste para selecionar a área (Esc ou botão direito cancela).".dim());
    }
    let Some(capture) = prepare::make_capture(&app.cfg, app.backend, spec)? else {
        println!("Cancelado.");
        return Ok(());
    };
    let plan = prepare::video_plan(&app.cfg, capture, audio, output);
    record(app, plan, now).map(|_| ())
}

fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

fn interactive(app: &mut App) -> Result<()> {
    ffmpeg::check_available(&app.cfg.ffmpeg)?;
    println!("{}", "WRecorder — gravador de tela e áudio".bold().cyan());
    loop {
        println!();
        let choice = Select::with_theme(&theme())
            .with_prompt("O que você quer gravar?")
            .items(["Tela  (vídeo MP4)", "Somente áudio  (WAV / MP3)", "Configurações", "Sair"])
            .default(0)
            .interact_opt()?;
        let result = match choice {
            Some(0) => menu_video(app),
            Some(1) => menu_audio(app),
            Some(2) => {
                println!("Arquivo de configuração: {}", Config::path().display().to_string().bold());
                Ok(())
            }
            _ => return Ok(()),
        };
        if let Err(e) = result {
            let _ = crossterm::terminal::disable_raw_mode();
            eprintln!("{} {e:#}", "erro:".red().bold());
        }
    }
}

fn ask_audio_source(prompt: &str, default: AudioSource, allow_none: bool) -> Result<Option<AudioSource>> {
    let mut opts = vec![AudioSource::Mic, AudioSource::System, AudioSource::Both];
    if allow_none {
        opts.insert(0, AudioSource::None);
    }
    let labels: Vec<&str> = opts.iter().map(|o| o.label()).collect();
    let def = opts.iter().position(|o| *o == default).unwrap_or(0);
    let idx = Select::with_theme(&theme()).with_prompt(prompt).items(&labels).default(def).interact_opt()?;
    Ok(idx.map(|i| opts[i]))
}

fn menu_video(app: &mut App) -> Result<()> {
    let idx = Select::with_theme(&theme())
        .with_prompt("Área de captura")
        .items(["Tela inteira", "Selecionar área", "Janela  (aplicativo / navegador)"])
        .default(0)
        .interact_opt()?;
    let spec = match idx {
        Some(0) => match choose_monitor(app, true)? {
            Some(m) => TargetSpec::Screen(m),
            None => return Ok(()),
        },
        Some(1) => match choose_monitor(app, false)? {
            Some(m) => TargetSpec::SelectRegion(m),
            None => return Ok(()),
        },
        Some(2) => match pick_window(app, None)? {
            Some(s) => s,
            None => return Ok(()),
        },
        _ => return Ok(()),
    };
    let Some(audio) = ask_audio_source("Áudio do vídeo", app.cfg.video.audio, true)? else { return Ok(()) };
    let now = app.cfg.start_immediately;
    run_video(app, spec, audio, None, now)
}

fn menu_audio(app: &mut App) -> Result<()> {
    let fmts = [AudioFormat::Mp3, AudioFormat::Wav];
    let def = fmts.iter().position(|f| *f == app.cfg.audio.format).unwrap_or(0);
    let Some(fi) = Select::with_theme(&theme())
        .with_prompt("Formato")
        .items(["MP3  (compacto)", "WAV  (sem perdas)"])
        .default(def)
        .interact_opt()?
    else {
        return Ok(());
    };
    let Some(source) = ask_audio_source("Fonte de áudio", app.cfg.audio.source, false)? else { return Ok(()) };
    let plan = prepare::audio_plan(&app.cfg, fmts[fi], source, None);
    let now = app.cfg.start_immediately;
    record(app, plan, now)?;
    Ok(())
}

/// `Some(None)` = todas as telas / padrão; `None` = cancelado.
fn choose_monitor(app: &App, allow_all: bool) -> Result<Option<Option<capture::Monitor>>> {
    if app.backend.is_wayland() {
        return Ok(Some(None));
    }
    let mons = capture::monitors()?;
    if mons.len() <= 1 {
        return Ok(Some(mons.into_iter().next()));
    }
    let mut labels: Vec<String> = mons
        .iter()
        .enumerate()
        .map(|(i, m)| format!("{i}: {}  {}{}", m.name, m.rect, if m.primary { "  (principal)" } else { "" }))
        .collect();
    if allow_all {
        labels.push("Todas as telas".into());
    }
    let def = mons.iter().position(|m| m.primary).unwrap_or(0);
    let Some(i) = Select::with_theme(&theme()).with_prompt("Monitor").items(&labels).default(def).interact_opt()? else {
        return Ok(None);
    };
    Ok(Some(mons.get(i).cloned()))
}

fn pick_window(app: &App, query: Option<&str>) -> Result<Option<TargetSpec>> {
    if app.backend.is_wayland() {
        if query.is_some() {
            println!("{}", "No Wayland a janela é escolhida no diálogo do sistema.".yellow());
        }
        return Ok(Some(TargetSpec::Window(None)));
    }
    let wins = capture::windows()?;
    if wins.is_empty() {
        bail!("nenhuma janela encontrada");
    }
    let chosen = if let Some(q) = query {
        let q = q.to_lowercase();
        wins.into_iter()
            .find(|w| w.title.to_lowercase().contains(&q) || w.app.to_lowercase().contains(&q))
            .ok_or_else(|| anyhow!("nenhuma janela contém '{q}' (veja `wrecorder list`)"))?
    } else {
        let labels: Vec<String> = wins.iter().map(|w| w.label()).collect();
        let Some(i) = FuzzySelect::with_theme(&theme())
            .with_prompt("Janela (digite para filtrar)")
            .items(&labels)
            .default(0)
            .interact_opt()?
        else {
            return Ok(None);
        };
        wins[i].clone()
    };
    println!("Janela: {}", chosen.title.clone().bold());
    Ok(Some(TargetSpec::Window(Some(chosen))))
}

struct RawGuard;
impl RawGuard {
    fn new() -> Result<RawGuard> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(RawGuard)
    }
}
impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn status_line(text: String) {
    let mut out = std::io::stdout();
    let _ = crossterm::execute!(out, crossterm::cursor::MoveToColumn(0), crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine));
    let _ = write!(out, "{text}");
    let _ = out.flush();
}

fn terminal_action(ev: Event) -> Option<Action> {
    let Event::Key(k) = ev else { return None };
    if k.kind != KeyEventKind::Press {
        return None;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        return Some(Action::Stop);
    }
    match k.code {
        KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => Some(Action::Start),
        KeyCode::Char(' ') | KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::Pause),
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Esc => Some(Action::Stop),
        _ => None,
    }
}

#[derive(PartialEq)]
enum State {
    Waiting,
    Recording,
    Paused,
}

fn record(app: &mut App, plan: Plan, now: bool) -> Result<Option<PathBuf>> {
    if app.hotkeys.is_none() {
        let wayland = app.backend.is_wayland();
        if wayland {
            println!("{}", "Registrando atalhos globais no sistema (pode aparecer uma confirmação)…".dim());
        }
        let (tx, rx) = mpsc::channel();
        let svc = HotkeyService::new(&app.cfg.hotkeys, wayland, tx);
        println!("{}", svc.status.clone().dim());
        app.hotkeys = Some((svc, rx));
    }
    let hk_rx = &app.hotkeys.as_ref().unwrap().1;
    while hk_rx.try_recv().is_ok() {}
    let keys = app.cfg.hotkeys.clone();

    let kind = match &plan.video {
        Some(v) => format!("vídeo MP4 · {} fps · {} · áudio: {}", v.fps, v.encoder, plan.audio.label()),
        None => format!("áudio {} · {}", plan.audio_format.ext().to_uppercase(), plan.audio.label()),
    };
    println!("{} {}", "▶".cyan(), kind);
    println!("  Salvar em: {}", plan.output.display());
    println!(
        "  Iniciar {} (ou Enter) · Pausar/retomar {} (ou Espaço) · Encerrar {} (ou Q) · Cancelar: Esc antes de iniciar",
        keys.start.as_str().bold(),
        keys.pause.as_str().bold(),
        keys.stop.as_str().bold()
    );

    let mut rec = Recorder::new(plan, app.cfg.clone())?;
    let raw = RawGuard::new()?;
    let mut state = State::Waiting;
    if now {
        rec.start()?;
        state = State::Recording;
    }
    let mut error: Option<anyhow::Error> = None;
    let mut blink = 0u32;
    let mut last_line = String::new();

    loop {
        hotkeys::pump_messages();
        let mut action = hk_rx.try_recv().ok();
        if action.is_none() && event::poll(Duration::from_millis(50))? {
            action = terminal_action(event::read()?);
        }
        match (&state, action) {
            (State::Waiting, Some(Action::Start)) | (State::Waiting, Some(Action::Pause)) => {
                rec.start()?;
                state = State::Recording;
            }
            (State::Waiting, Some(Action::Stop)) => {
                drop(raw);
                println!("\r\nCancelado.");
                rec.discard();
                return Ok(None);
            }
            (State::Recording, Some(Action::Pause)) => {
                status_line(format!("{} pausando…", "⏸".yellow()));
                if let Err(e) = rec.pause() {
                    error = Some(e);
                    break;
                }
                state = State::Paused;
            }
            (State::Paused, Some(Action::Pause)) | (State::Paused, Some(Action::Start)) => {
                rec.start()?;
                state = State::Recording;
            }
            (_, Some(Action::Stop)) => break,
            _ => {}
        }
        if let Err(e) = rec.check_health() {
            error = Some(e);
            break;
        }
        blink += 1;
        let t = fmt_dur(rec.elapsed());
        let line = match state {
            State::Waiting => format!("{} aguardando — pressione {} ou Enter para iniciar", "●".dim(), keys.start),
            State::Recording => {
                let dot = if (blink / 10) % 2 == 0 { "●".red().bold() } else { "●".dark_red() };
                format!("{dot} GRAVANDO  {}   ({} pausa · {} encerra)", t.bold(), keys.pause, keys.stop)
            }
            State::Paused => format!("{} PAUSADO   {}   ({} retoma · {} encerra)", "⏸".yellow().bold(), t.bold(), keys.pause, keys.stop),
        };
        if line != last_line {
            status_line(line.clone());
            last_line = line;
        }
    }

    status_line(format!("{} finalizando…", "■".cyan()));
    let result = if rec.has_content() { rec.finish() } else { Err(anyhow!("nada foi gravado")) };
    drop(raw);
    println!();
    if let Some(e) = error {
        eprintln!("{} {e:#}", "erro durante a gravação:".red().bold());
    }
    let path = result?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("{} Salvo: {}  ({:.1} MB)", "✔".green().bold(), path.display().to_string().bold(), size as f64 / 1_048_576.0);
    Ok(Some(path))
}

fn list(app: &App) -> Result<()> {
    if app.backend.is_wayland() {
        println!("Sessão Wayland: monitores e janelas são escolhidos no diálogo do sistema (portal).");
        return Ok(());
    }
    println!("{}", "Monitores".bold());
    for (i, m) in capture::monitors()?.iter().enumerate() {
        println!("  {i}: {}  {}{}", m.name, m.rect, if m.primary { "  (principal)" } else { "" });
    }
    println!("{}", "Janelas".bold());
    for w in capture::windows()? {
        println!("  {}", w.label());
    }
    Ok(())
}
