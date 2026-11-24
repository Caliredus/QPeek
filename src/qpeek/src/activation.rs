use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::accounts;

const MINUTES_PER_DAY: u32 = 24 * 60;
const CATCH_UP_MINUTES: u32 = 60;
const COOLDOWN_SECONDS: i64 = 295 * 60;
const TRIGGER_TIMEOUT: Duration = Duration::from_secs(45);
const CLI_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    #[default]
    OnReset,
    Schedule,
}

impl ActivationMode {
    pub fn as_int(self) -> i32 {
        match self {
            Self::OnReset => 0,
            Self::Schedule => 1,
        }
    }

    pub fn from_int(value: i32) -> Self {
        match value {
            1 => Self::Schedule,
            _ => Self::OnReset,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MasterSchedule {
    pub enabled: bool,
    pub mode: ActivationMode,
    pub start_time: String,
    pub quiet_enabled: bool,
    pub quiet_start: String,
    pub quiet_end: String,
}

impl Default for MasterSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ActivationMode::OnReset,
            start_time: "08:00".into(),
            quiet_enabled: true,
            quiet_start: "23:00".into(),
            quiet_end: "07:30".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSchedule {
    pub enabled: bool,
    pub follow_master: bool,
    pub mode: ActivationMode,
    pub start_time: String,
    pub quiet_enabled: bool,
    pub quiet_start: String,
    pub quiet_end: String,
    pub last_triggered_unix: Option<i64>,
    pub last_status: String,
}

impl Default for AccountSchedule {
    fn default() -> Self {
        let master = MasterSchedule::default();
        Self {
            enabled: false,
            follow_master: true,
            mode: master.mode,
            start_time: master.start_time,
            quiet_enabled: master.quiet_enabled,
            quiet_start: master.quiet_start,
            quiet_end: master.quiet_end,
            last_triggered_unix: None,
            last_status: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivationConfig {
    pub master: MasterSchedule,
    pub accounts: HashMap<String, AccountSchedule>,
}

pub fn supports_activation(provider: &str) -> bool {
    matches!(provider, "codex" | "claude" | "antigravity")
}

pub fn account_id(provider: &str, key: &str) -> String {
    format!("{provider}:{key}")
}

pub fn step_hhmm(hhmm: &str, delta_minutes: i32) -> String {
    let minutes = i64::from(parse_hhmm(hhmm).unwrap_or_default()) + i64::from(delta_minutes);
    let minutes = minutes.rem_euclid(i64::from(MINUTES_PER_DAY));
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

pub fn parse_hhmm(s: &str) -> Option<u32> {
    let [h1, h2, b':', m1, m2] = *s.as_bytes() else {
        return None;
    };
    if ![h1, h2, m1, m2].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hours = u32::from((h1 - b'0') * 10 + (h2 - b'0'));
    let minutes = u32::from((m1 - b'0') * 10 + (m2 - b'0'));
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

pub fn is_in_quiet_hours(now_mins: u32, start: &str, end: &str) -> bool {
    let (Some(start), Some(end)) = (parse_hhmm(start), parse_hhmm(end)) else {
        return false;
    };
    if now_mins >= MINUTES_PER_DAY {
        return false;
    }
    if start <= end {
        (start..end).contains(&now_mins)
    } else {
        now_mins >= start || now_mins < end
    }
}

pub fn effective_schedule<'a>(
    master: &'a MasterSchedule,
    acc: &'a AccountSchedule,
) -> (ActivationMode, &'a str, bool, &'a str, &'a str) {
    if acc.follow_master {
        (
            master.mode,
            &master.start_time,
            master.quiet_enabled,
            &master.quiet_start,
            &master.quiet_end,
        )
    } else {
        (
            acc.mode,
            &acc.start_time,
            acc.quiet_enabled,
            &acc.quiet_start,
            &acc.quiet_end,
        )
    }
}

pub fn should_auto_trigger(
    cfg: &ActivationConfig,
    provider: &str,
    key: &str,
    has_5h: bool,
    used_5h_pct: Option<f32>,
    now_local_mins: u32,
    now_utc_unix: i64,
) -> bool {
    if !cfg.master.enabled
        || !supports_activation(provider)
        || !has_5h
        || now_local_mins >= MINUTES_PER_DAY
    {
        return false;
    }
    let Some(acc) = cfg.accounts.get(&account_id(provider, key)) else {
        return false;
    };
    if !acc.enabled {
        return false;
    }

    let (mode, start_time, quiet_enabled, quiet_start, quiet_end) =
        effective_schedule(&cfg.master, acc);
    if quiet_enabled && is_in_quiet_hours(now_local_mins, quiet_start, quiet_end) {
        return false;
    }
    if acc
        .last_triggered_unix
        .is_some_and(|last| now_utc_unix.saturating_sub(last) < COOLDOWN_SECONDS)
    {
        return false;
    }
    if !used_5h_pct.is_some_and(|percent| percent <= 0.5) {
        return false;
    }

    match mode {
        ActivationMode::OnReset => true,
        ActivationMode::Schedule => parse_hhmm(start_time).is_some_and(|target| {
            let elapsed = (now_local_mins + MINUTES_PER_DAY - target) % MINUTES_PER_DAY;
            elapsed < CATCH_UP_MINUTES
        }),
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("QPeek").join("activation.json"))
}

pub fn load() -> ActivationConfig {
    config_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &ActivationConfig) {
    let (Some(path), Ok(bytes)) = (config_path(), serde_json::to_vec_pretty(cfg)) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!("activation.{}.{id}.tmp", std::process::id()));
    let Ok(mut file) = fs::File::create_new(&temp) else {
        return;
    };
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if written.and_then(|()| fs::rename(&temp, path)).is_err() {
        let _ = fs::remove_file(temp);
    }
}

fn find_cli(exe: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH");
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let fallback_paths = home
        .iter()
        .flat_map(|home| fallbacks.iter().map(move |fallback| home.join(fallback)));
    path.iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(exe))
        .chain(fallback_paths)
        .find(|path| path.is_file())
}

pub fn trigger_now(provider: &str, account_key: &str) -> Result<(), String> {
    let (exe_name, fallbacks, args): (&str, &[&str], &[&str]) = match provider {
        "codex" => (
            "codex.exe",
            &["AppData/Local/Programs/OpenAI/Codex/bin/codex.exe"],
            &[
                "exec",
                "--ephemeral",
                "-s",
                "read-only",
                "-c",
                r#"model_reasoning_effort="low""#,
                "--skip-git-repo-check",
                "Reply OK",
            ],
        ),
        "claude" => (
            "claude.exe",
            &[".local/bin/claude.exe"],
            &[
                "-p",
                "--no-session-persistence",
                "--model",
                "haiku",
                "Reply OK",
            ],
        ),
        "antigravity" => (
            "agy.exe",
            &[
                "AppData/Local/agy/bin/agy.exe",
                "AppData/Local/Programs/Antigravity/bin/agy.exe",
            ],
            &[
                "-p",
                "--disable-slash-commands",
                "--effort",
                "low",
                "--print-timeout",
                "30s",
                "Reply OK",
            ],
        ),
        _ => return Err("Bu sağlayıcı oturum başlatmayı desteklemiyor".into()),
    };
    let exe = find_cli(exe_name, fallbacks).ok_or_else(|| format!("{exe_name} bulunamadı"))?;
    let mut cmd = Command::new(exe);
    if account_key != accounts::DEFAULT_KEY {
        let home_env = accounts::home_env(provider).ok_or("Hesap ortam değişkeni bulunamadı")?;
        let extra = accounts::extras(provider)
            .into_iter()
            .find(|extra| extra.key == account_key)
            .ok_or("Hesap bulunamadı")?;
        cmd.env(home_env, extra.dir);
    }

    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CLAUDECODE",
    ] {
        cmd.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|error| format!("{exe_name} başlatılamadı: {error}"))?;
    let started = Instant::now();
    let error = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{exe_name} hata verdi ({status})")),
            Ok(None) => {
                let remaining = TRIGGER_TIMEOUT.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    break format!("{exe_name} zaman aşımına uğradı");
                }
                std::thread::sleep(remaining.min(CLI_POLL_INTERVAL));
            }
            Err(error) => break format!("{exe_name} beklenemedi: {error}"),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DEFAULT_ACCOUNT_ID: &str = "codex:default";

    fn enabled_config() -> ActivationConfig {
        let mut cfg = ActivationConfig::default();
        cfg.master.enabled = true;
        cfg.accounts.insert(
            account_id("codex", accounts::DEFAULT_KEY),
            AccountSchedule {
                enabled: true,
                ..AccountSchedule::default()
            },
        );
        cfg
    }

    fn triggers_at(cfg: &ActivationConfig, time: &str) -> bool {
        should_auto_trigger(
            cfg,
            "codex",
            accounts::DEFAULT_KEY,
            true,
            Some(0.0),
            parse_hhmm(time).unwrap(),
            NOW,
        )
    }

    #[test]
    fn mode_converts_to_and_from_ui_values() {
        assert_eq!(ActivationMode::OnReset.as_int(), 0);
        assert_eq!(ActivationMode::Schedule.as_int(), 1);
        assert_eq!(ActivationMode::from_int(0), ActivationMode::OnReset);
        assert_eq!(ActivationMode::from_int(1), ActivationMode::Schedule);
        assert_eq!(ActivationMode::from_int(-1), ActivationMode::OnReset);
    }

    #[test]
    fn partial_config_preserves_opt_in_defaults() {
        let json = r#"{"master":{"mode":"schedule"},"accounts":{"claude:extra":{}}}"#;
        let cfg: ActivationConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.master.enabled);
        assert_eq!(cfg.master.mode, ActivationMode::Schedule);
        assert_eq!(cfg.master.start_time, "08:00");
        assert!(cfg.master.quiet_enabled);
        assert_eq!(cfg.master.quiet_start, "23:00");
        assert_eq!(cfg.master.quiet_end, "07:30");
        let acc = &cfg.accounts["claude:extra"];
        assert!(!acc.enabled);
        assert!(acc.follow_master);
        assert_eq!(acc.mode, ActivationMode::OnReset);
        assert_eq!(acc.last_triggered_unix, None);
        assert!(acc.last_status.is_empty());
        let serialized = serde_json::to_value(&cfg).unwrap();
        assert_eq!(serialized["master"]["mode"], "schedule");
        assert_eq!(serialized["accounts"]["claude:extra"]["mode"], "on_reset");
    }

    #[test]
    fn parses_only_valid_hhmm() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("07:30"), Some(450));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        for invalid in ["24:00", "12:60", "7:30", "07:3", " 07:30", "+7:30", "α:30"] {
            assert_eq!(parse_hhmm(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn steps_time_with_midnight_wraparound() {
        assert_eq!(step_hhmm("23:30", 30), "00:00");
        assert_eq!(step_hhmm("00:00", -30), "23:30");
        assert_eq!(step_hhmm("08:00", 1440 + 30), "08:30");
        assert_eq!(step_hhmm("08:00", i32::MAX), "10:07");
        assert_eq!(step_hhmm("invalid", 30), "00:30");
    }

    #[test]
    fn quiet_hours_include_the_start_and_exclude_the_end() {
        assert!(!is_in_quiet_hours(13 * 60 - 1, "13:00", "15:00"));
        assert!(is_in_quiet_hours(13 * 60, "13:00", "15:00"));
        assert!(is_in_quiet_hours(15 * 60 - 1, "13:00", "15:00"));
        assert!(!is_in_quiet_hours(15 * 60, "13:00", "15:00"));
        assert!(!is_in_quiet_hours(13 * 60, "13:00", "13:00"));
        assert!(!is_in_quiet_hours(13 * 60, "invalid", "15:00"));
    }

    #[test]
    fn quiet_hours_can_span_midnight() {
        assert!(!is_in_quiet_hours(23 * 60 - 1, "23:00", "07:30"));
        assert!(is_in_quiet_hours(23 * 60, "23:00", "07:30"));
        assert!(is_in_quiet_hours(0, "23:00", "07:30"));
        assert!(is_in_quiet_hours(7 * 60 + 29, "23:00", "07:30"));
        assert!(!is_in_quiet_hours(7 * 60 + 30, "23:00", "07:30"));
    }

    #[test]
    fn cooldown_blocks_recent_and_future_triggers() {
        let mut cfg = enabled_config();
        for (last, expected) in [
            (NOW, false),
            (NOW - COOLDOWN_SECONDS + 1, false),
            (NOW - COOLDOWN_SECONDS, true),
            (NOW + 1, false),
        ] {
            cfg.accounts
                .get_mut(DEFAULT_ACCOUNT_ID)
                .unwrap()
                .last_triggered_unix = Some(last);
            assert_eq!(triggers_at(&cfg, "08:00"), expected);
        }
    }

    #[test]
    fn requires_a_supported_enabled_account_and_an_idle_five_hour_window() {
        let mut cfg = enabled_config();
        assert!(supports_activation("antigravity"));
        for provider in ["claude", "antigravity", "gemini"] {
            cfg.accounts.insert(
                account_id(provider, accounts::DEFAULT_KEY),
                AccountSchedule {
                    enabled: true,
                    ..AccountSchedule::default()
                },
            );
        }
        for (provider, key, has_5h, used, expected) in [
            ("codex", "default", false, Some(0.0), false),
            ("codex", "default", true, None, false),
            ("codex", "default", true, Some(0.51), false),
            ("codex", "default", true, Some(f32::NAN), false),
            ("codex", "default", true, Some(0.5), true),
            ("codex", "other", true, Some(0.0), false),
            ("claude", "default", true, Some(0.0), true),
            ("antigravity", "default", true, Some(0.0), true),
            ("antigravity", "default", false, Some(0.0), false),
            ("gemini", "default", true, Some(0.0), false),
        ] {
            assert_eq!(
                should_auto_trigger(&cfg, provider, key, has_5h, used, 8 * 60, NOW),
                expected,
            );
        }
        cfg.master.enabled = false;
        assert!(!triggers_at(&cfg, "08:00"));
        cfg.master.enabled = true;
        cfg.accounts.get_mut(DEFAULT_ACCOUNT_ID).unwrap().enabled = false;
        assert!(!triggers_at(&cfg, "08:00"));
    }

    #[test]
    fn custom_schedule_overrides_master_mode_time_and_quiet_hours() {
        let mut cfg = enabled_config();
        let acc = cfg.accounts.get_mut(DEFAULT_ACCOUNT_ID).unwrap();
        acc.mode = ActivationMode::Schedule;
        acc.start_time = "01:00".into();
        acc.quiet_enabled = false;
        acc.quiet_start = "12:00".into();
        acc.quiet_end = "14:00".into();
        assert_eq!(
            effective_schedule(&cfg.master, acc),
            (ActivationMode::OnReset, "08:00", true, "23:00", "07:30"),
        );
        assert!(!triggers_at(&cfg, "01:15"));
        assert!(triggers_at(&cfg, "08:00"));

        let acc = cfg.accounts.get_mut(DEFAULT_ACCOUNT_ID).unwrap();
        acc.follow_master = false;
        assert_eq!(
            effective_schedule(&cfg.master, acc),
            (ActivationMode::Schedule, "01:00", false, "12:00", "14:00"),
        );
        assert!(triggers_at(&cfg, "01:15"));
        assert!(!triggers_at(&cfg, "08:00"));
    }

    #[test]
    fn scheduled_catch_up_window_is_sixty_minutes_and_can_span_midnight() {
        let mut cfg = enabled_config();
        cfg.master.mode = ActivationMode::Schedule;
        cfg.master.quiet_enabled = false;
        for (time, expected) in [
            ("07:59", false),
            ("08:00", true),
            ("08:59", true),
            ("09:00", false),
        ] {
            assert_eq!(triggers_at(&cfg, time), expected, "{time}");
        }
        cfg.master.start_time = "23:30".into();
        for (time, expected) in [
            ("23:29", false),
            ("23:30", true),
            ("00:00", true),
            ("00:29", true),
            ("00:30", false),
        ] {
            assert_eq!(triggers_at(&cfg, time), expected, "{time}");
        }
        cfg.master.quiet_enabled = true;
        assert!(!triggers_at(&cfg, "00:00"));
        cfg.master.start_time = "invalid".into();
        assert!(!triggers_at(&cfg, "08:00"));
    }
}
