//! Contexto compartilhado pelas páginas: banco, runtime, janela e servidor dos celulares.
//!
//! Cada página (`src/pages/*.rs`) recebe um [`AppCtx`] no `bind` e guarda o próprio estado num
//! `thread_local!` dela (só a thread da interface mexe). Assim as áreas evoluem sem editar o mesmo arquivo.

use std::future::Future;

use slint::{ComponentHandle, SharedString, Weak};
use tokio::runtime::Handle;

use barerp_db::events::Event;
use barerp_db::Db;
use barerp_server::ServerHandle;

use crate::AppWindow;

#[derive(Clone)]
pub struct AppCtx {
    pub db: Db,
    pub rt: Handle,
    pub ui: Weak<AppWindow>,
    pub server: Option<ServerHandle>,
}

impl AppCtx {
    pub fn new(ui: &AppWindow, db: Db, rt: Handle, server: Option<ServerHandle>) -> Self {
        Self { db, rt, ui: ui.as_weak(), server }
    }

    pub fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    /// Roda `fut` no tokio e entrega o resultado para `done` na thread da interface.
    pub fn spawn<T, F, D>(&self, fut: F, done: D)
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
        D: FnOnce(&AppWindow, T) + Send + 'static,
    {
        let ui = self.ui.clone();
        self.rt.spawn(async move {
            let result = fut.await;
            let _ = ui.upgrade_in_event_loop(move |w| done(&w, result));
        });
    }

    /// Avisa celulares e telas de cozinha (Socket.IO) do que mudou.
    pub fn emit(&self, events: &[Event]) {
        if let Some(server) = &self.server {
            let (server, events) = (server.clone(), events.to_vec());
            self.rt.spawn(async move { server.emit(&events).await });
        }
    }

    /// Mensagem no rodapé (vermelha quando `error`).
    pub fn status(&self, message: impl Into<SharedString>, error: bool) {
        if let Some(ui) = self.ui() {
            ui.set_status_message(message.into());
            ui.set_status_error(error);
        }
    }

    /// Usuário logado (nome), para auditoria e "aberto por".
    pub fn user_name(&self) -> Option<String> {
        crate::controller::logged_user_name()
    }
}

// ---------- Preferências do usuário neste PC (tema e escala de fonte) ----------

use std::cell::Cell;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Escala de fonte do original: Normal (100%), Grande (125%), Extra Grande (140%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontScale {
    #[default]
    Normal,
    Large,
    XLarge,
}

impl FontScale {
    pub fn factor(self) -> f32 {
        match self {
            FontScale::Normal => 1.0,
            FontScale::Large => 1.2,
            FontScale::XLarge => 1.4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FontScale::Normal => "A",
            FontScale::Large => "A+",
            FontScale::XLarge => "A++",
        }
    }

    /// Etiqueta do botão "Aumentar Tamanho" da Retaguarda (como no original)
    pub fn multiplier_label(self) -> &'static str {
        match self {
            FontScale::Normal => "1x",
            FontScale::Large => "1.2x",
            FontScale::XLarge => "1.4x",
        }
    }

    pub fn next(self) -> Self {
        match self {
            FontScale::Normal => FontScale::Large,
            FontScale::Large => FontScale::XLarge,
            FontScale::XLarge => FontScale::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub dark: bool,
    pub font_scale: FontScale,
    /// "Auto-Impressão de Cozinha" da Retaguarda: imprime a comanda dos pedidos novos neste PC
    pub auto_print_kitchen: bool,
    /// Aviso sonoro do KDS quando chega item novo
    pub kds_sound: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { dark: true, font_scale: FontScale::Normal, auto_print_kitchen: false, kds_sound: true }
    }
}

fn preferences_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("BarERP Pro").join("preferencias.json")
}

impl Preferences {
    pub fn load() -> Self {
        std::fs::read_to_string(preferences_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = preferences_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

thread_local! {
    /// Escala do sistema (DPI do monitor) antes do zoom do usuário.
    static BASE_SCALE: Cell<Option<f32>> = const { Cell::new(None) };
    /// Zoom escolhido ("Aumentar Tamanho") e a escala que o app aplicou por último na janela.
    static ZOOM: Cell<FontScale> = const { Cell::new(FontScale::Normal) };
    static APPLIED_SCALE: Cell<Option<f32>> = const { Cell::new(None) };
}

/// Aplica tema e escala de fonte na janela (o zoom vale para a tela inteira, como no original).
pub fn apply_preferences(ui: &AppWindow, prefs: Preferences) {
    ui.global::<crate::Theme>().set_dark(prefs.dark);
    ui.set_font_scale_label(prefs.font_scale.label().into());
    ui.global::<crate::RetaguardaStore>().set_font_label(prefs.font_scale.multiplier_label().into());
    ui.global::<crate::RetaguardaStore>().set_auto_print_kitchen(prefs.auto_print_kitchen);
    ui.global::<crate::KdsStore>().set_sound(prefs.kds_sound);
    ZOOM.with(|z| z.set(prefs.font_scale));
    sync_zoom(ui);
}

/// Aplica o zoom na janela e a remonta na largura nova, como o navegador fazia no original.
///
/// O Slint só troca a escala: sem o tamanho lógico novo a tela continuava montada para a largura antiga e o
/// que passava da janela ficava cortado. Também é chamada pelo timer da casca (4 vezes por segundo): antes de
/// a janela existir não faz nada, e quando o Windows redefine a escala (ao criar a janela ou trocar de monitor,
/// apagando o zoom) ela vira a nova escala base e o zoom é reaplicado.
pub fn sync_zoom(ui: &AppWindow) {
    let window = ui.window();
    let size = window.size();
    if size.width == 0 || size.height == 0 {
        return;
    }
    let current = window.scale_factor();
    let ours = APPLIED_SCALE.with(|a| a.get()).is_some_and(|a| (a - current).abs() < 0.001);
    let base = if ours { BASE_SCALE.with(|b| b.get()).unwrap_or(current) } else { current };
    BASE_SCALE.with(|b| b.set(Some(base)));
    let wanted = base * ZOOM.with(|z| z.get()).factor();
    APPLIED_SCALE.with(|a| a.set(Some(wanted)));
    if (wanted - current).abs() < 0.001 {
        return;
    }
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: wanted });
    window.dispatch_event(slint::platform::WindowEvent::Resized { size: size.to_logical(wanted) });
}

/// Abre um endereço no navegador padrão do sistema (WhatsApp, links externos).
pub fn open_external(url: &str) {
    // rundll32 recebe a URL inteira: o `cmd /C start` quebrava os links com `&` e `%` (texto do WhatsApp)
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        tracing::warn!("não foi possível abrir {url}: {e}");
    }
}

/// Copia o texto para a área de transferência do sistema (ID da máquina, chave PIX, links).
pub fn copy_text(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let child = Command::new("pbcopy").stdin(Stdio::piped()).spawn();
    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        // Sem janela de console piscando (CREATE_NO_WINDOW)
        Command::new("clip").stdin(Stdio::piped()).creation_flags(0x0800_0000).spawn()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let child = Command::new("xclip").args(["-selection", "clipboard"]).stdin(Stdio::piped()).spawn();
    if let Ok(mut child) = child {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// Aviso sonoro curto (pedido novo no KDS), sem travar a tela: no Windows o bipe do console (sai na caixa de
/// som), no macOS um som do sistema e no Linux o som de alerta do PulseAudio, se houver.
pub fn beep() {
    std::thread::spawn(|| {
        #[cfg(windows)]
        let result = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", "[console]::beep(1046,180);[console]::beep(1318,260)"])
                .creation_flags(0x0800_0000)
                .status()
        };
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("afplay").arg("/System/Library/Sounds/Glass.aiff").status();
        #[cfg(all(unix, not(target_os = "macos")))]
        let result = std::process::Command::new("paplay").arg("/usr/share/sounds/freedesktop/stereo/bell.oga").status();
        if let Err(e) = result {
            tracing::debug!("sem aviso sonoro: {e}");
        }
    });
}
