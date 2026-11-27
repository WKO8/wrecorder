//! Motor de gravação.
//!
//! Cada trecho entre "iniciar/retomar" e "pausar/encerrar" vira um segmento gravado
//! por um processo ffmpeg. Ao encerrar, os segmentos são concatenados sem
//! recodificação (`-c copy`) — pausar não custa qualidade nem tempo.

use crate::audio::{self, AudioInput, AudioKind};
use crate::capture::{Rect, VideoTarget};
use crate::config::{AudioFormat, AudioSource, Config};
use crate::ffmpeg;
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// De onde vem o vídeo.
pub enum Capture {
    /// Linux X11 via x11grab.
    #[cfg(target_os = "linux")]
    X11 { display: String, target: VideoTarget },
    /// Linux Wayland via portal + PipeWire (+ recorte opcional, em pixels do stream).
    #[cfg(target_os = "linux")]
    Portal { session: crate::wayland::PortalSession, crop: Option<Rect> },
    /// Windows via gdigrab.
    #[cfg(windows)]
    Gdi { target: VideoTarget },
}

pub struct VideoPlan {
    pub capture: Capture,
    pub fps: u32,
    pub encoder: String,
}

pub struct Plan {
    /// `None` = somente áudio.
    pub video: Option<VideoPlan>,
    pub audio: AudioSource,
    /// Formato quando é somente áudio.
    pub audio_format: AudioFormat,
    pub output: PathBuf,
}

struct Running {
    ffmpeg: Child,
    gst: Option<Child>,
    _audio: Vec<AudioInput>,
    file: PathBuf,
    log: PathBuf,
}

pub struct Recorder {
    plan: Plan,
    cfg: Config,
    workdir: PathBuf,
    segments: Vec<PathBuf>,
    running: Option<Running>,
    active_since: Option<Instant>,
    accumulated: Duration,
}

impl Recorder {
    pub fn new(plan: Plan, cfg: Config) -> Result<Recorder> {
        let parent = plan.output.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&parent).with_context(|| format!("criando {}", parent.display()))?;
        let stem = plan.output.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let workdir = parent.join(format!(".{stem}.parts"));
        std::fs::create_dir_all(&workdir)?;
        Ok(Recorder {
            plan,
            cfg,
            workdir,
            segments: Vec::new(),
            running: None,
            active_since: None,
            accumulated: Duration::ZERO,
        })
    }

    pub fn has_content(&self) -> bool {
        !self.segments.is_empty() || self.running.is_some()
    }

    /// Tempo gravado (sem contar pausas).
    pub fn elapsed(&self) -> Duration {
        self.accumulated + self.active_since.map(|t| t.elapsed()).unwrap_or_default()
    }

    fn segment_ext(&self) -> &'static str {
        if self.plan.video.is_some() {
            "mkv"
        } else {
            self.plan.audio_format.ext()
        }
    }

    /// Inicia (ou retoma) a gravação.
    pub fn start(&mut self) -> Result<()> {
        if self.running.is_some() {
            return Ok(());
        }
        let idx = self.segments.len();
        let r = self.spawn_segment(idx)?;
        self.running = Some(r);
        self.active_since = Some(Instant::now());
        Ok(())
    }

    /// Pausa: fecha o segmento atual.
    pub fn pause(&mut self) -> Result<()> {
        let Some(mut r) = self.running.take() else { return Ok(()) };
        if let Some(t) = self.active_since.take() {
            self.accumulated += t.elapsed();
        }
        ffmpeg::stop_gracefully(&mut r.ffmpeg, Duration::from_secs(10));
        if let Some(mut g) = r.gst.take() {
            let _ = g.kill();
            let _ = g.wait();
        }
        drop(r._audio);
        let ok = std::fs::metadata(&r.file).map(|m| m.len() > 0).unwrap_or(false);
        if ok {
            self.segments.push(r.file);
            Ok(())
        } else {
            bail!("o trecho não foi gravado:\n{}", ffmpeg::log_tail(&r.log, 10))
        }
    }

    /// Verifica se o ffmpeg do segmento atual continua vivo.
    pub fn check_health(&mut self) -> Result<()> {
        if let Some(r) = self.running.as_mut() {
            if let Ok(Some(status)) = r.ffmpeg.try_wait() {
                let log = r.log.clone();
                let file = r.file.clone();
                if let Some(mut g) = r.gst.take() {
                    let _ = g.kill();
                    let _ = g.wait();
                }
                self.running = None;
                if let Some(t) = self.active_since.take() {
                    self.accumulated += t.elapsed();
                }
                if std::fs::metadata(&file).map(|m| m.len() > 0).unwrap_or(false) {
                    self.segments.push(file);
                }
                bail!("o ffmpeg parou inesperadamente ({status}):\n{}", ffmpeg::log_tail(&log, 12));
            }
        }
        Ok(())
    }

    /// Encerra e gera o arquivo final.
    pub fn finish(mut self) -> Result<PathBuf> {
        let pause_result = self.pause();
        if self.segments.is_empty() {
            pause_result?;
            bail!("nada foi gravado");
        }
        if let Err(e) = pause_result {
            eprintln!("Aviso: o último trecho falhou: {e:#}");
        }
        let mp4 = self.plan.video.is_some();
        ffmpeg::concat(&self.cfg.ffmpeg, &self.segments, &self.plan.output, mp4, &self.workdir)
            .with_context(|| format!("os trechos brutos foram mantidos em {}", self.workdir.display()))?;
        let _ = std::fs::remove_dir_all(&self.workdir);
        Ok(self.plan.output.clone())
    }

    /// Cancela tudo e apaga os arquivos temporários.
    pub fn discard(mut self) {
        let _ = self.pause();
        let _ = std::fs::remove_dir_all(&self.workdir);
    }

    #[allow(unused_mut, unused_assignments)]
    fn spawn_segment(&self, idx: usize) -> Result<Running> {
        let file = self.workdir.join(format!("seg_{idx:03}.{}", self.segment_ext()));
        let log = self.workdir.join(format!("seg_{idx:03}.log"));
        let cfg = &self.cfg;

        let mut cmd = Command::new(&cfg.ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "warning", "-nostats", "-y"]);
        #[cfg(unix)]
        cmd.arg("-nostdin");

        let mut inputs = 0usize;
        let mut gst: Option<Child> = None;
        let mut stdin = Stdio::null();
        let mut crop: Option<Rect> = None;

        // ---------------- vídeo
        if let Some(v) = &self.plan.video {
            let fps = v.fps.to_string();
            let cursor = if cfg.video.show_cursor { "1" } else { "0" };
            match &v.capture {
                #[cfg(target_os = "linux")]
                Capture::X11 { display, target } => {
                    cmd.args(["-thread_queue_size", "1024", "-f", "x11grab", "-framerate", &fps, "-draw_mouse", cursor]);
                    match target {
                        VideoTarget::Screen(None) => {
                            cmd.args(["-i", display]);
                        }
                        VideoTarget::Screen(Some(m)) => x11_rect(&mut cmd, display, m.rect),
                        VideoTarget::Region(r) => x11_rect(&mut cmd, display, *r),
                        VideoTarget::Window(w) => {
                            cmd.args(["-window_id", &format!("0x{:x}", w.id), "-i", display]);
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                Capture::Portal { session, crop: c } => {
                    let fd = session.open_remote()?;
                    let mut g = Command::new(&cfg.gst_launch);
                    g.args(crate::wayland::gst_args(session.node_id, v.fps, false))
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(File::create(self.workdir.join(format!("seg_{idx:03}.gst.log")))?);
                    crate::wayland::pass_fd_as_3(&mut g, &fd);
                    ffmpeg::detach(&mut g);
                    let mut child = g.spawn().with_context(|| {
                        format!("executando '{}' (instale gstreamer1.0-pipewire e gstreamer1.0-plugins-good)", cfg.gst_launch)
                    })?;
                    drop(fd);
                    stdin = Stdio::from(child.stdout.take().expect("stdout do gst"));
                    gst = Some(child);
                    cmd.args(["-thread_queue_size", "1024", "-f", "yuv4mpegpipe", "-i", "pipe:0"]);
                    crop = *c;
                }
                #[cfg(windows)]
                Capture::Gdi { target } => {
                    cmd.args(["-thread_queue_size", "1024", "-f", "gdigrab", "-framerate", &fps, "-draw_mouse", cursor]);
                    let rect_args = |cmd: &mut Command, r: Rect| {
                        let r = r.even();
                        cmd.args(["-offset_x", &r.x.to_string(), "-offset_y", &r.y.to_string()])
                            .args(["-video_size", &format!("{}x{}", r.w, r.h), "-i", "desktop"]);
                    };
                    match target {
                        VideoTarget::Screen(None) => {
                            cmd.args(["-i", "desktop"]);
                        }
                        VideoTarget::Screen(Some(m)) => rect_args(&mut cmd, m.rect),
                        VideoTarget::Region(r) => rect_args(&mut cmd, *r),
                        VideoTarget::Window(w) => {
                            // O título muda (ex.: abas do navegador): relê pelo HWND a cada trecho.
                            let title = crate::capture::win::window_title(w.id).unwrap_or_else(|| w.title.clone());
                            cmd.args(["-i", &format!("title={title}")]);
                        }
                    }
                }
            }
            inputs += 1;
        }

        // ---------------- áudio
        let mut kinds = Vec::new();
        if self.plan.audio.has_mic() {
            kinds.push(AudioKind::Mic);
        }
        if self.plan.audio.has_system() {
            kinds.push(AudioKind::System);
        }
        let mut audio_inputs = Vec::new();
        let audio_first = inputs;
        for k in &kinds {
            let ai = audio::open(*k, &cfg.audio)?;
            cmd.args(&ai.args);
            audio_inputs.push(ai);
        }

        // ---------------- filtros e mapeamento
        let mut graph: Vec<String> = Vec::new();
        let mut maps: Vec<String> = Vec::new();
        if self.plan.video.is_some() {
            let mut chain = Vec::new();
            if let Some(c) = crop {
                let c = c.even();
                chain.push(format!("crop={}:{}:{}:{}", c.w, c.h, c.x, c.y));
            }
            chain.push("crop=trunc(iw/2)*2:trunc(ih/2)*2".to_string());
            graph.push(format!("[0:v]{}[v]", chain.join(",")));
            maps.push("[v]".into());
        }
        // Cada entrada de áudio vira um rótulo; o microfone passa pela redução de ruído.
        let mut labels: Vec<String> = Vec::new();
        for (n, kind) in kinds.iter().enumerate() {
            let idx = audio_first + n;
            match (kind, cfg.audio.noise_reduction.filter()) {
                (AudioKind::Mic, Some(f)) => {
                    graph.push(format!("[{idx}:a]{f}[mic]"));
                    labels.push("[mic]".into());
                }
                _ => labels.push(format!("[{idx}:a]")),
            }
        }
        match labels.len() {
            0 => {}
            1 if labels[0] == "[mic]" => maps.push("[mic]".into()),
            1 => maps.push(format!("{audio_first}:a")),
            n => {
                graph.push(format!("{}amix=inputs={n}:duration=longest:dropout_transition=0:normalize=0[a]", labels.concat()));
                maps.push("[a]".into());
            }
        }
        if !graph.is_empty() {
            cmd.args(["-filter_complex", &graph.join(";")]);
        }
        for m in &maps {
            cmd.args(["-map", m]);
        }

        // ---------------- codificação
        let rate = cfg.audio.sample_rate.to_string();
        if let Some(v) = &self.plan.video {
            cmd.args(ffmpeg::video_codec_args(&v.encoder, cfg.video.quality, &cfg.video.x264_preset, v.fps));
            if !audio_inputs.is_empty() {
                cmd.args(["-c:a", "aac", "-b:a", &format!("{}k", cfg.video.audio_bitrate), "-ar", &rate, "-ac", "2"]);
            }
            cmd.args(["-f", "matroska"]);
        } else {
            match self.plan.audio_format {
                AudioFormat::Wav => cmd.args(["-c:a", "pcm_s16le", "-ar", &rate, "-ac", "2", "-f", "wav"]),
                AudioFormat::Mp3 => cmd.args([
                    "-c:a", "libmp3lame", "-b:a", &format!("{}k", cfg.audio.mp3_bitrate),
                    "-ar", &rate, "-ac", "2", "-f", "mp3",
                ]),
            };
        }
        cmd.arg(&file);

        #[cfg(windows)]
        {
            stdin = Stdio::piped(); // para enviar 'q' ao encerrar
        }
        cmd.stdin(stdin).stdout(Stdio::null()).stderr(File::create(&log)?);
        ffmpeg::detach(&mut cmd);
        std::fs::write(self.workdir.join(format!("seg_{idx:03}.cmd")), format!("{cmd:?}")).ok();

        let ffmpeg_child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Some(mut g) = gst {
                    let _ = g.kill();
                }
                return Err(e).with_context(|| format!("executando '{}'", cfg.ffmpeg));
            }
        };
        Ok(Running { ffmpeg: ffmpeg_child, gst, _audio: audio_inputs, file, log })
    }
}

#[cfg(target_os = "linux")]
fn x11_rect(cmd: &mut Command, display: &str, r: Rect) {
    let r = r.even();
    cmd.args(["-video_size", &format!("{}x{}", r.w, r.h), "-i", &format!("{display}+{},{}", r.x, r.y)]);
}
