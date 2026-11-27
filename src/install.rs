//! Instala o WRecorder no menu de aplicativos do sistema (somente para o usuário atual,
//! sem precisar de administrador).

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

#[cfg(not(windows))]
const ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
  <rect x="4" y="4" width="120" height="120" rx="28" fill="#202126"/>
  <circle cx="64" cy="64" r="40" fill="#f0f0f0"/>
  <circle cx="64" cy="64" r="32" fill="#e53935"/>
</svg>
"##;

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("localizando o executável")
}

#[cfg(not(windows))]
fn paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("pasta pessoal não encontrada"))?;
    let data = dirs::data_dir().unwrap_or_else(|| home.join(".local/share"));
    Ok((
        home.join(".local/bin/wrecorder"),
        data.join("icons/hicolor/scalable/apps/wrecorder.svg"),
        data.join("applications/wrecorder.desktop"),
    ))
}

#[cfg(not(windows))]
pub fn install() -> Result<PathBuf> {
    let (bin, icon, desktop) = paths()?;
    for p in [&bin, &icon, &desktop] {
        std::fs::create_dir_all(p.parent().unwrap())?;
    }
    let src = exe()?;
    if src.canonicalize().ok() != bin.canonicalize().ok() {
        // Copia para um arquivo temporário e renomeia: funciona mesmo com o app aberto.
        let tmp = bin.with_extension("new");
        std::fs::copy(&src, &tmp).with_context(|| format!("copiando para {}", bin.display()))?;
        std::fs::rename(&tmp, &bin)?;
    }
    std::fs::write(&icon, ICON_SVG)?;
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=WRecorder\n\
         GenericName=Gravador de tela e áudio\n\
         Comment=Grave a tela em MP4 e áudio em WAV ou MP3\n\
         Exec={}\n\
         Icon=wrecorder\n\
         Terminal=false\n\
         Categories=AudioVideo;Video;Audio;Recorder;\n\
         Keywords=gravar;gravador;tela;screen;recorder;audio;video;captura;\n\
         StartupWMClass=wrecorder\n\
         StartupNotify=true\n",
        bin.display()
    );
    std::fs::write(&desktop, entry)?;
    let apps = desktop.parent().unwrap();
    let _ = std::process::Command::new("update-desktop-database").arg(apps).output();
    let icons = icon.ancestors().nth(3).unwrap().to_path_buf();
    let _ = std::process::Command::new("gtk-update-icon-cache").args(["-f", "-t"]).arg(&icons).output();
    Ok(desktop)
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<()> {
    let (bin, icon, desktop) = paths()?;
    for p in [bin, icon, desktop] {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

#[cfg(windows)]
fn win_paths() -> Result<(PathBuf, PathBuf)> {
    let local = dirs::data_local_dir().ok_or_else(|| anyhow!("LOCALAPPDATA não encontrado"))?;
    let roaming = dirs::data_dir().ok_or_else(|| anyhow!("APPDATA não encontrado"))?;
    Ok((
        local.join("Programs").join("WRecorder").join("wrecorder.exe"),
        roaming.join(r"Microsoft\Windows\Start Menu\Programs\WRecorder.lnk"),
    ))
}

#[cfg(windows)]
pub fn install() -> Result<PathBuf> {
    let (bin, lnk) = win_paths()?;
    std::fs::create_dir_all(bin.parent().unwrap())?;
    let src = exe()?;
    if src.canonicalize().ok() != bin.canonicalize().ok() {
        std::fs::copy(&src, &bin).with_context(|| format!("copiando para {}", bin.display()))?;
    }
    let ps = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.Description='Gravador de tela e áudio';$s.Save()",
        lnk.display().to_string().replace('\'', "''"),
        bin.display().to_string().replace('\'', "''"),
        bin.parent().unwrap().display().to_string().replace('\'', "''"),
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .status()
        .context("executando powershell")?;
    if !status.success() {
        return Err(anyhow!("não foi possível criar o atalho no Menu Iniciar"));
    }
    Ok(lnk)
}

#[cfg(windows)]
pub fn uninstall() -> Result<()> {
    let (bin, lnk) = win_paths()?;
    let _ = std::fs::remove_file(lnk);
    let _ = std::fs::remove_file(bin);
    Ok(())
}
