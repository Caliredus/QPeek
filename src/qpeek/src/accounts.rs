//! Hesaplar: her sağlayıcının bir "ana" hesabı (bilgisayardaki resmî giriş: ~/.codex,
//! ~/.claude, agy) ve Codex/Claude için QPeek'in kendi klasöründe tuttuğu ek hesaplar.
//!
//! Kural: QPeek hiçbir resmî girişten çıkış yapmaz. Ana hesabı "kaldırmak" yalnız
//! çekirdeğin onu okumasını kapatır (`config disable`); ek hesabı kaldırmak QPeek'in
//! kendi klasörünü siler. Giriş bilgilerinden yalnız e-posta alanı okunur.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
pub const DEFAULT_KEY: &str = "default";

/// Ek hesap destekleyen sağlayıcılar (çekirdek ortam değişkeniyle ayrı klasör okuyabiliyor).
pub fn supports_extra(provider: &str) -> bool {
    matches!(provider, "codex" | "claude")
}

/// Çekirdeği bu hesabın klasörüne yönlendiren ortam değişkeni.
pub fn home_env(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("CODEX_HOME"),
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Extra {
    pub provider: String,
    pub key: String,
    pub dir: PathBuf,
}

fn root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("QPeek").join("accounts"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// QPeek'in eklediği hesaplar (yalnız girişi tamamlanmış klasörler).
pub fn extras(provider: &str) -> Vec<Extra> {
    let Some(dir) = root().map(|r| r.join(provider)) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Extra> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| has_login(provider, &e.path()))
        .map(|e| Extra {
            provider: provider.to_string(),
            key: e.file_name().to_string_lossy().into_owned(),
            dir: e.path(),
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn has_login(provider: &str, dir: &Path) -> bool {
    match provider {
        "codex" => dir.join("auth.json").is_file(),
        "claude" => dir.join(".credentials.json").is_file(),
        _ => false,
    }
}

/// Bilgisayardaki resmî giriş var mı?
pub fn default_logged_in(provider: &str) -> bool {
    let Some(h) = home() else { return false };
    match provider {
        "codex" => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| h.join(".codex"))
            .join("auth.json")
            .is_file(),
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| h.join(".claude"))
            .join(".credentials.json")
            .is_file(),
        // agy'nin giriş dosyası belgeli değil; çekirdek veri döndürüyorsa bağlı sayılır.
        "antigravity" => true,
        _ => false,
    }
}

/// Ana hesabın e-postası.
pub fn default_email(provider: &str) -> Option<String> {
    let h = home()?;
    match provider {
        "codex" => codex_email(
            &std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| h.join(".codex")),
        ),
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .and_then(|d| claude_email(&d.join(".claude.json")))
            .or_else(|| claude_email(&h.join(".claude.json"))),
        _ => None,
    }
}

pub fn extra_email(extra: &Extra) -> Option<String> {
    match extra.provider.as_str() {
        "codex" => codex_email(&extra.dir),
        "claude" => claude_email(&extra.dir.join(".claude.json")),
        _ => None,
    }
}

/// Codex: auth.json → tokens.id_token (JWT) → payload.email. Token'ın kendisi kullanılmaz.
fn codex_email(codex_home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(codex_home.join("auth.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let jwt = json.get("tokens")?.get("id_token")?.as_str()?;
    let payload = base64url_decode(jwt.split('.').nth(1)?)?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    claims.get("email")?.as_str().map(str::to_string)
}

/// Claude: .claude.json → oauthAccount.emailAddress.
fn claude_email(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("oauthAccount")?.get("emailAddress")?.as_str().map(str::to_string)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

fn which(exe: &str, fallbacks: &[PathBuf]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        .chain(fallbacks.iter().cloned())
        .find(|p| p.is_file())
}

/// Resmî CLI ile yeni bir hesaba tarayıcıdan giriş yapar, QPeek klasörüne kaydeder.
/// Giriş bitene (ya da 5 dk dolana) kadar bekler; bu yüzden ayrı iş parçacığında çağrılır.
pub fn login(provider: &str) -> Result<(), String> {
    let env = home_env(provider).ok_or("Bu sağlayıcıya ek hesap bağlanamıyor")?;
    let h = home().ok_or("Kullanıcı klasörü bulunamadı")?;
    let (exe, args): (PathBuf, &[&str]) = match provider {
        "codex" => (
            which("codex.exe", &[h.join("AppData/Local/Programs/OpenAI/Codex/bin/codex.exe")])
                .ok_or("Codex CLI bulunamadı")?,
            &["login"],
        ),
        "claude" => (
            which("claude.exe", &[h.join(".local/bin/claude.exe")]).ok_or("Claude Code bulunamadı")?,
            &["auth", "login", "--claudeai"],
        ),
        _ => return Err("Bu sağlayıcıya ek hesap bağlanamıyor".into()),
    };

    let base = root().ok_or("Uygulama klasörü bulunamadı")?.join(provider);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir = base.join(format!("{stamp:x}"));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env(env, &dir)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Girişin tarayıcı aboneliğiyle yapılması için devralınan API kimliklerini temizle.
    for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY", "CLAUDECODE"] {
        cmd.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let result = (|| {
        let mut child = cmd.spawn().map_err(|e| format!("Giriş başlatılamadı: {e}"))?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(s)) if s.success() => break,
                Ok(Some(s)) => return Err(format!("Giriş tamamlanmadı (çıkış {})", s.code().unwrap_or(-1))),
                Ok(None) if started.elapsed() > LOGIN_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Giriş zaman aşımına uğradı".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => return Err(e.to_string()),
            }
        }
        if has_login(provider, &dir) { Ok(()) } else { Err("Giriş bilgisi kaydedilmedi".into()) }
    })();
    if result.is_err() {
        remove_dir_safely(provider, &dir);
    }
    result
}

/// Ek hesabı kaldırır: yalnız QPeek'in kendi hesap klasörünü siler.
pub fn remove_extra(provider: &str, key: &str) -> Result<(), String> {
    let base = root().ok_or("Uygulama klasörü bulunamadı")?.join(provider);
    let dir = base.join(key);
    if key.is_empty() || key.contains(['/', '\\', '.']) || !dir.is_dir() {
        return Err("Hesap bulunamadı".into());
    }
    remove_dir_safely(provider, &dir);
    if dir.exists() { Err("Hesap klasörü silinemedi".into()) } else { Ok(()) }
}

/// Yalnız `%APPDATA%\QPeek\accounts\<sağlayıcı>\` altındaki bir klasörü siler.
fn remove_dir_safely(provider: &str, dir: &Path) {
    let Some(base) = root().map(|r| r.join(provider)) else { return };
    if let (Ok(base), Ok(dir)) = (base.canonicalize(), dir.canonicalize())
        && dir.parent() == Some(base.as_path())
    {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_jwt_payload() {
        // {"email":"a@b.c"} → base64url
        let payload = "eyJlbWFpbCI6ImFAYi5jIn0";
        let bytes = base64url_decode(payload).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), r#"{"email":"a@b.c"}"#);
    }
}
