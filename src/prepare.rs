//! Transforma a escolha do usuário (tela / área / janela) em uma `Capture` pronta,
//! independente de a interface ser a GUI ou o terminal.

use crate::capture::{self, Monitor, Rect, VideoTarget, WindowInfo};
use crate::config::{AudioFormat, AudioSource, Config};
use crate::ffmpeg;
use crate::recorder::{Capture, Plan, VideoPlan};
use crate::selector;
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    #[cfg(target_os = "linux")]
    X11,
    #[cfg(target_os = "linux")]
    Wayland,
    #[cfg(windows)]
    Windows,
}

impl Backend {
    pub fn detect(cfg: &Config) -> Backend {
        #[cfg(target_os = "linux")]
        {
            return match cfg.linux_backend.to_lowercase().as_str() {
                "x11" => Backend::X11,
                "wayland" => Backend::Wayland,
                _ if crate::wayland::is_wayland_session() => Backend::Wayland,
                _ => Backend::X11,
            };
        }
        #[cfg(windows)]
        {
            let _ = cfg;
            Backend::Windows
        }
    }

    /// No Wayland o sistema (portal) escolhe monitor/janela; não dá para listar.
    pub fn is_wayland(self) -> bool {
        #[cfg(target_os = "linux")]
        return self == Backend::Wayland;
        #[allow(unreachable_code)]
        false
    }
}

/// O que o usuário quer gravar.
#[derive(Clone, Debug)]
pub enum TargetSpec {
    /// Monitor (None = todas as telas; no Wayland, o portal pergunta).
    Screen(Option<Monitor>),
    /// Selecionar a área com o mouse sobre este monitor (None = principal / portal).
    SelectRegion(Option<Monitor>),
    /// Área fixa em coordenadas da área de trabalho.
    Region(Rect),
    /// Janela (None = escolher no portal do Wayland).
    Window(Option<WindowInfo>),
}

/// Prepara a captura. Pode mostrar o diálogo do sistema (Wayland) ou o seletor de área.
/// Retorna `None` se o usuário cancelar.
pub fn make_capture(cfg: &Config, backend: Backend, spec: TargetSpec) -> Result<Option<Capture>> {
    #[cfg(target_os = "linux")]
    if backend.is_wayland() {
        use crate::wayland::{PortalSession, PortalSource};
        let cursor = cfg.video.show_cursor;
        return Ok(match spec {
            TargetSpec::Screen(_) => Some(Capture::Portal { session: PortalSession::start(PortalSource::Monitor, cursor)?, crop: None }),
            TargetSpec::Window(_) => Some(Capture::Portal { session: PortalSession::start(PortalSource::Window, cursor)?, crop: None }),
            TargetSpec::Region(r) => Some(Capture::Portal { session: PortalSession::start(PortalSource::Monitor, cursor)?, crop: Some(r) }),
            TargetSpec::SelectRegion(_) => {
                let session = PortalSession::start(PortalSource::Monitor, cursor)?;
                let shot = TempFile::new("y4m");
                screenshot_portal(cfg, &shot.0, session.node_id, session.open_remote()?)?;
                match selector::select_region(&shot.0, None)? {
                    Some(r) => Some(Capture::Portal { session, crop: Some(r) }),
                    None => None,
                }
            }
        });
    }
    let _ = backend;

    let target = match spec {
        TargetSpec::Screen(m) => VideoTarget::Screen(m),
        TargetSpec::Region(r) => VideoTarget::Region(r),
        TargetSpec::Window(Some(w)) => VideoTarget::Window(w),
        TargetSpec::Window(None) => bail!("escolha uma janela"),
        TargetSpec::SelectRegion(m) => {
            let mon = match m {
                Some(m) => m,
                None => {
                    let mons = capture::monitors()?;
                    mons.iter().find(|m| m.primary).or(mons.first()).cloned().ok_or_else(|| anyhow!("nenhum monitor"))?
                }
            };
            let shot = TempFile::new("y4m");
            screenshot_native(cfg, &shot.0, mon.rect)?;
            let Some(r) = selector::select_region(&shot.0, Some((mon.rect.x, mon.rect.y)))? else { return Ok(None) };
            VideoTarget::Region(Rect { x: r.x + mon.rect.x, y: r.y + mon.rect.y, ..r })
        }
    };
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        return Ok(Some(Capture::X11 { display, target }));
    }
    #[cfg(windows)]
    return Ok(Some(Capture::Gdi { target }));
}

struct TempFile(PathBuf);
impl TempFile {
    fn new(ext: &str) -> TempFile {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        TempFile(std::env::temp_dir().join(format!("wrecorder-{}-{n}.{ext}", std::process::id())))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(target_os = "linux")]
fn screenshot_portal(cfg: &Config, dest: &Path, node: u32, fd: std::os::fd::OwnedFd) -> Result<()> {
    use anyhow::Context as _;
    use std::time::Duration;
    let mut cmd = Command::new(&cfg.gst_launch);
    cmd.args(crate::wayland::gst_args(node, 1, true))
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(dest)?)
        .stderr(std::fs::File::create(dest.with_extension("log"))?);
    crate::wayland::pass_fd_as_3(&mut cmd, &fd);
    let mut child = cmd.spawn().with_context(|| format!("executando {}", cfg.gst_launch))?;
    drop(fd);
    let start = std::time::Instant::now();
    loop {
        if let Some(st) = child.try_wait()? {
            let log = std::fs::read_to_string(dest.with_extension("log")).unwrap_or_default();
            let _ = std::fs::remove_file(dest.with_extension("log"));
            if !st.success() {
                let detail: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
                bail!(
                    "não foi possível capturar a tela via PipeWire: {}",
                    detail[detail.len().saturating_sub(3)..].join(" | ")
                );
            }
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            bail!("tempo esgotado capturando a tela");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn screenshot_native(cfg: &Config, dest: &Path, rect: Rect) -> Result<()> {
    let r = rect.even();
    let mut cmd = Command::new(&cfg.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        cmd.args(["-f", "x11grab", "-video_size", &format!("{}x{}", r.w, r.h)])
            .args(["-i", &format!("{display}+{},{}", r.x, r.y)]);
    }
    #[cfg(windows)]
    cmd.args(["-f", "gdigrab", "-offset_x", &r.x.to_string(), "-offset_y", &r.y.to_string()])
        .args(["-video_size", &format!("{}x{}", r.w, r.h), "-i", "desktop"]);
    cmd.args(["-frames:v", "1", "-pix_fmt", "yuv444p", "-f", "yuv4mpegpipe"]).arg(dest);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    ffmpeg::detach(&mut cmd);
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("falha ao capturar a tela: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn output_path(cfg: &Config, given: Option<PathBuf>, video: bool, ext: &str) -> PathBuf {
    let name = format!("wrecorder_{}.{ext}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    match given {
        Some(p) if p.is_dir() => p.join(name),
        Some(p) if p.extension().is_none() => p.with_extension(ext),
        Some(p) => p,
        None => cfg.output_dir(video).join(name),
    }
}

pub fn video_plan(cfg: &Config, capture: Capture, audio: AudioSource, output: Option<PathBuf>) -> Plan {
    let encoder = ffmpeg::resolve_encoder(&cfg.ffmpeg, &cfg.video.encoder);
    Plan {
        video: Some(VideoPlan { capture, fps: cfg.video.fps, encoder }),
        audio,
        audio_format: cfg.audio.format,
        output: output_path(cfg, output, true, "mp4"),
    }
}

pub fn audio_plan(cfg: &Config, format: AudioFormat, source: AudioSource, output: Option<PathBuf>) -> Plan {
    Plan { video: None, audio: source, audio_format: format, output: output_path(cfg, output, false, format.ext()) }
}

/// Abre a pasta do arquivo no gerenciador de arquivos.
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let _ = Command::new("xdg-open").arg(dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

/// Abre o arquivo no reprodutor padrão.
pub fn open_file(path: &Path) {
    #[cfg(windows)]
    {
        let _ = Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}
