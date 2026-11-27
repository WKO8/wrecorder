//! Utilitários do ffmpeg: detecção de encoder, controle de processo e concatenação.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn check_available(ffmpeg: &str) -> Result<()> {
    let ok = Command::new(ffmpeg)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        bail!(
            "ffmpeg não encontrado ('{ffmpeg}'). Instale-o (Linux: `sudo apt install ffmpeg`; \
             Windows: `winget install Gyan.FFmpeg`) ou ajuste o caminho em {}",
            crate::config::Config::path().display()
        );
    }
    Ok(())
}

/// Testa rapidamente se um encoder funciona de verdade (driver/GPU presentes).
fn encoder_works(ffmpeg: &str, enc: &str) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x240:r=30"])
        .args(["-frames:v", "3", "-pix_fmt", "yuv420p", "-c:v", enc, "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve "auto" para o melhor encoder H.264 disponível (GPU primeiro).
pub fn resolve_encoder(ffmpeg: &str, configured: &str) -> String {
    if configured != "auto" {
        return configured.to_string();
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["h264_nvenc", "h264_qsv", "h264_amf"]
    } else {
        &["h264_nvenc"]
    };
    for enc in candidates {
        if encoder_works(ffmpeg, enc) {
            return enc.to_string();
        }
    }
    "libx264".to_string()
}

/// Argumentos de codificação de vídeo para cada encoder.
pub fn video_codec_args(encoder: &str, quality: u32, x264_preset: &str, fps: u32) -> Vec<String> {
    let q = quality.clamp(0, 51).to_string();
    let mut a: Vec<String> = vec!["-c:v".into(), encoder.into()];
    let push = |a: &mut Vec<String>, xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match encoder {
        "libx264" | "libx265" => push(&mut a, &["-preset", x264_preset, "-crf", &q, "-pix_fmt", "yuv420p"]),
        e if e.contains("nvenc") => push(&mut a, &["-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", &q, "-b:v", "0", "-pix_fmt", "yuv420p"]),
        e if e.contains("qsv") => push(&mut a, &["-global_quality", &q, "-pix_fmt", "nv12"]),
        e if e.contains("amf") => push(&mut a, &["-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-pix_fmt", "yuv420p"]),
        _ => push(&mut a, &["-pix_fmt", "yuv420p"]),
    }
    push(&mut a, &["-g", &(fps * 2).to_string()]);
    a
}

/// Prepara um Command para rodar em segundo plano sem receber sinais do terminal.
pub fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Pede ao ffmpeg para finalizar o arquivo (SIGINT no Unix, 'q' no stdin no Windows)
/// e espera até `timeout`; mata o processo se não responder.
pub fn stop_gracefully(child: &mut Child, timeout: Duration) {
    #[cfg(unix)]
    unsafe {
        libc_kill(child.id() as i32, 2 /* SIGINT */);
    }
    #[cfg(windows)]
    {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        drop(child.stdin.take());
    }
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, sig);
}

/// Últimas linhas de um log (para mensagens de erro).
pub fn log_tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let v: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

/// Junta os segmentos (sem recodificar) no arquivo final.
pub fn concat(ffmpeg: &str, segments: &[PathBuf], output: &Path, mp4: bool, workdir: &Path) -> Result<()> {
    if segments.is_empty() {
        bail!("nenhum trecho foi gravado");
    }
    if segments.len() == 1 && !mp4 {
        // Áudio com um único trecho: basta mover.
        if std::fs::rename(&segments[0], output).is_ok() {
            return Ok(());
        }
    }
    let list = workdir.join("concat.txt");
    let mut body = String::new();
    for s in segments {
        let p = s.canonicalize().unwrap_or_else(|_| s.clone());
        let p = p.to_string_lossy().replace('\\', "/").replace('\'', r"'\''");
        body.push_str(&format!("file '{p}'\n"));
    }
    std::fs::write(&list, body)?;
    let log = workdir.join("concat.log");
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy"]);
    if mp4 {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log)?);
    detach(&mut cmd);
    let status = cmd.status().context("executando ffmpeg (concat)")?;
    if !status.success() {
        bail!("falha ao montar o arquivo final:\n{}", log_tail(&log, 8));
    }
    Ok(())
}
