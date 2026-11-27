//! Captura no Wayland via xdg-desktop-portal (ScreenCast) + PipeWire.
//!
//! O portal mostra o seletor nativo do sistema (monitor ou janela). A sessão fica
//! aberta durante toda a gravação; cada segmento (após pausar/retomar) pede um novo
//! descritor do PipeWire, sem mostrar o diálogo de novo.

use anyhow::{anyhow, Context, Result};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use std::os::fd::OwnedFd;
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalSource {
    Monitor,
    Window,
}

enum Cmd {
    OpenRemote(mpsc::Sender<Result<OwnedFd>>),
}

/// Sessão de screencast ativa. Fechada quando dropada.
pub struct PortalSession {
    pub node_id: u32,
    pub serial: Option<u64>,
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
}

impl PortalSession {
    pub fn start(source: PortalSource, cursor: bool) -> Result<PortalSession> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, Option<u64>)>>();

        thread::Builder::new().name("portal-screencast".into()).spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.into()));
                    return;
                }
            };
            rt.block_on(async move {
                let setup = async {
                    let proxy = Screencast::new().await?;
                    let session = proxy.create_session(Default::default()).await?;
                    let modes = proxy.available_cursor_modes().await.unwrap_or(CursorMode::EMBEDDED);
                    let cursor_mode = if cursor && modes.contains(CursorMode::EMBEDDED) {
                        CursorMode::EMBEDDED
                    } else {
                        CursorMode::HIDDEN
                    };
                    let types = match source {
                        PortalSource::Monitor => SourceType::MONITOR,
                        PortalSource::Window => SourceType::WINDOW,
                    };
                    proxy
                        .select_sources(
                            &session,
                            SelectSourcesOptions::default()
                                .set_cursor_mode(cursor_mode)
                                .set_sources(types)
                                .set_multiple(false)
                                .set_persist_mode(PersistMode::DoNot),
                        )
                        .await?;
                    let streams = proxy.start(&session, None, Default::default()).await?.response()?;
                    let stream = streams.streams().first().cloned().ok_or_else(|| {
                        anyhow!("nenhuma fonte selecionada no portal")
                    })?;
                    anyhow::Ok((proxy, session, stream))
                };
                let (proxy, session, stream) = match setup.await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.context("portal de captura de tela (cancelado?)")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok((stream.pipe_wire_node_id(), stream.pipewire_serial())));

                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        Cmd::OpenRemote(reply) => {
                            let r = proxy
                                .open_pipe_wire_remote(&session, Default::default())
                                .await
                                .map_err(anyhow::Error::from);
                            let _ = reply.send(r);
                        }
                    }
                }
                let _ = session.close().await;
            });
        })?;

        let (node_id, serial) = ready_rx
            .recv()
            .map_err(|_| anyhow!("thread do portal terminou inesperadamente"))??;
        Ok(PortalSession { node_id, serial, tx })
    }

    /// Novo descritor do PipeWire para este screencast.
    pub fn open_remote(&self) -> Result<OwnedFd> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Cmd::OpenRemote(tx))
            .map_err(|_| anyhow!("sessão do portal encerrada"))?;
        rx.recv().context("sessão do portal encerrada")?
    }
}

/// Monta o comando gst-launch que lê o PipeWire (fd 3 no filho) e escreve Y4M no stdout.
pub fn gst_args(node_id: u32, fps: u32, single_frame: bool) -> Vec<String> {
    let mut src = format!("pipewiresrc fd=3 path={node_id} do-timestamp=true always-copy=true");
    if single_frame {
        src.push_str(" num-buffers=1");
    } else {
        // Reenvia o último quadro com a tela parada (o GNOME só envia quadros quando algo muda).
        src.push_str(" keepalive-time=100");
    }
    let caps = if single_frame {
        // y4menc do GStreamer 1.28 não aceita Y444; I420 funciona em todas as versões.
        "video/x-raw,format=I420".to_string()
    } else {
        format!("video/x-raw,format=I420,framerate={fps}/1")
    };
    let pipeline = if single_frame {
        format!("{src} ! videoconvert ! {caps} ! y4menc ! fdsink fd=1")
    } else {
        format!("{src} ! videoconvert ! videorate ! {caps} ! y4menc ! fdsink fd=1 sync=false")
    };
    let mut args = vec!["-q".to_string(), "-e".to_string()];
    args.extend(pipeline.split(' ').map(str::to_string));
    args
}

/// Prepara `cmd` para herdar `fd` como descritor 3.
pub fn pass_fd_as_3(cmd: &mut std::process::Command, fd: &OwnedFd) {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let raw = fd.as_raw_fd();
    unsafe {
        cmd.pre_exec(move || {
            if raw == 3 {
                let flags = libc::fcntl(3, libc::F_GETFD);
                libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            } else if libc::dup2(raw, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub fn is_wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").map(|s| s == "wayland").unwrap_or(false)
}

/// `wrecorder diagnose`: abre o portal e testa variações do pipeline do GStreamer,
/// mostrando o erro real de cada uma.
pub fn diagnose(gst: &str) -> Result<()> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let dir = std::env::temp_dir().join("wrecorder-diagnose");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    println!("Escolha uma tela no diálogo do sistema e clique em Compartilhar…");
    let s = PortalSession::start(PortalSource::Monitor, true)?;
    println!("portal ok: node {}  serial {:?}", s.node_id, s.serial);
    let node = s.node_id.to_string();
    let mut variants: Vec<(&str, Vec<String>)> = vec![
        ("frame", gst_args(s.node_id, 1, true)),
        ("stream", gst_args(s.node_id, 30, false)),
        ("minimal", "-e pipewiresrc fd=3 path=NODE num-buffers=10 ! videoconvert ! video/x-raw,format=I420 ! y4menc ! fdsink fd=1"
            .replace("NODE", &node).split(' ').map(String::from).collect()),
    ];
    if let Some(serial) = s.serial {
        variants.push(("serial", format!("-e pipewiresrc fd=3 target-object={serial} num-buffers=10 ! videoconvert ! video/x-raw,format=I420 ! y4menc ! fdsink fd=1")
            .split(' ').map(String::from).collect()));
    }
    for (name, args) in variants {
        let fd = s.open_remote()?;
        let out = dir.join(format!("{name}.y4m"));
        let err = dir.join(format!("{name}.log"));
        let mut cmd = Command::new(gst);
        cmd.args(&args)
            .env("GST_DEBUG", "3,pipewiresrc:5")
            .env("GST_DEBUG_NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&out)?)
            .stderr(std::fs::File::create(&err)?);
        pass_fd_as_3(&mut cmd, &fd);
        let mut child = cmd.spawn()?;
        drop(fd);
        let t = Instant::now();
        let status = loop {
            if let Some(st) = child.try_wait()? {
                break format!("{st}");
            }
            if t.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                break "rodando (encerrado após 5s)".to_string();
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        println!("\n== {name}: {status}, {bytes} bytes de vídeo");
        let log = std::fs::read_to_string(&err).unwrap_or_default();
        for l in log.lines().filter(|l| l.contains("ERROR") || l.contains("WARN") || l.contains("error")).take(12) {
            println!("   {}", l.chars().take(220).collect::<String>());
        }
    }
    println!("\nLogs completos em {}", dir.display());
    Ok(())
}
