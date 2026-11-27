//! Win-CodexBar çekirdeğiyle köprü: `codexbar-cli.exe usage --json` çalıştırılır,
//! stdout'taki JSON okunur. stderr (ör. Chrome ABE uyarıları) yok sayılır.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::accounts;
use crate::i18n::Lang;

include!(concat!(env!("OUT_DIR"), "/provider_meta.rs"));

const CORE_EXE: &str = "codexbar-cli.exe";
const CORE_TIMEOUT: Duration = Duration::from_secs(90);

/// Popüler sağlayıcıların kart sırası.
pub const POPULAR_ORDER: &[&str] = &[
    "codex",
    "claude",
    "antigravity",
    "gemini",
    "copilot",
    "cursor",
    "windsurf",
    "openrouter",
    "deepseek",
    "openaiapi",
    "grok",
    "xai",
    "mistral",
    "perplexity",
    "ollama",
    "groq",
    "jetbrains",
    "kiro",
    "opencode",
    "bedrock",
    "vertexai",
    "huggingface",
    "vercel",
];

pub fn provider_sort_rank(id: &str) -> usize {
    POPULAR_ORDER
        .iter()
        .position(|p| *p == id)
        .unwrap_or(POPULAR_ORDER.len())
}

/// API anahtarı ile bağlanan sağlayıcılar (`codexbar-cli config set-api-key --stdin`).
pub fn uses_api_key(id: &str) -> bool {
    api_key_meta(id).is_some()
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub provider: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, rename = "errorKind")]
    pub error_kind: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub pace: Option<PaceGroup>,
}

#[derive(Debug, Deserialize)]
pub struct PaceGroup {
    #[serde(default)]
    pub primary: Option<PaceInfo>,
    #[serde(default)]
    pub secondary: Option<PaceInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceInfo {
    #[serde(default)]
    pub delta_percent: f64,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub will_last_to_reset: bool,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub login_method: Option<String>,
    #[serde(default)]
    pub primary: Option<RateWindow>,
    #[serde(default)]
    pub secondary: Option<RateWindow>,
    #[serde(default)]
    pub tertiary: Option<RateWindow>,
    #[serde(default)]
    pub primary_label: Option<String>,
    #[serde(default)]
    pub secondary_label: Option<String>,
    #[serde(default)]
    pub tertiary_label: Option<String>,
    #[serde(default)]
    pub extra_rate_windows: Vec<ExtraWindow>,
}

#[derive(Debug, Deserialize)]
pub struct ExtraWindow {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub window: RateWindow,
}

#[derive(Debug, Deserialize)]
pub struct RateWindow {
    #[serde(default)]
    pub used_percent: f64,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reset_description: Option<String>,
    #[serde(default)]
    pub window_minutes: Option<u64>,
    #[serde(default)]
    pub is_informational: bool,
}

/// Kartın bir satırı: yüzde çubuğu ya da (info=true) sağda hap etiketli bilgi satırı.
/// TR ve EN metinleri birlikte tutulur, böylece dil değişince veri yeniden çekilmez.
#[derive(Debug, Clone)]
pub struct Metric {
    pub label_tr: String,
    pub label_en: String,
    pub percent: f32,
    pub detail_tr: String,
    pub detail_en: String,
    pub pace_tr: String,
    pub pace_en: String,
    pub pace_level: i32,
    pub info: bool,
    pub window_minutes: Option<u64>,
    /// Pencerenin sıfırlanma anı (çekirdekten geldiyse); limits.json'a yazılır.
    pub resets_at: Option<DateTime<Utc>>,
}

impl Metric {
    pub fn label(&self, lang: Lang) -> &str {
        match lang {
            Lang::Tr => &self.label_tr,
            Lang::En => &self.label_en,
        }
    }

    pub fn detail(&self, lang: Lang) -> &str {
        match lang {
            Lang::Tr => &self.detail_tr,
            Lang::En => &self.detail_en,
        }
    }

    pub fn pace(&self, lang: Lang) -> &str {
        match lang {
            Lang::Tr => &self.pace_tr,
            Lang::En => &self.pace_en,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub email: String,
    pub account_key: String,
    pub plan: String,
    pub metrics: Vec<Metric>,
    pub failed: bool,
}

impl Card {
    pub fn bars(&self) -> impl Iterator<Item = &Metric> {
        self.metrics.iter().filter(|m| !m.info)
    }

    pub fn has_5h_window(&self) -> bool {
        self.primary_5h_percent().is_some()
    }

    pub fn primary_5h_percent(&self) -> Option<f32> {
        if self.failed {
            return None;
        }
        self.bars()
            .find(|metric| metric.window_minutes == Some(300))
            .map(|metric| metric.percent)
    }
}

/// Çekirdeği önce QPeek.exe'nin yanında, sonra üst klasörlerdeki `dist/` içinde arar.
pub fn find_core() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let beside = dir.join(CORE_EXE);
    if beside.is_file() {
        return Some(beside);
    }
    dir.ancestors()
        .map(|d| d.join("dist").join(CORE_EXE))
        .find(|p| p.is_file())
}

pub fn fetch() -> Result<Vec<Card>, String> {
    let now = Utc::now();
    let extras: Vec<(&str, accounts::Extra)> = ["codex", "claude"]
        .into_iter()
        .filter(|id| accounts::supports_extra(id))
        .flat_map(|p| accounts::extras(p).into_iter().map(move |e| (p, e)))
        .collect();

    // Ana hesaplar ve ek hesaplar paralel çekilir; toplam bekleme süresi en yavaş çağrı kadar olur.
    let (default_res, extra_cards) = std::thread::scope(|s| {
        let default_handle = s.spawn(|| {
            run_core(&["usage", "--json"], None, None)
                .and_then(|j| parse(&j))
                .map(|entries| {
                    entries
                        .into_iter()
                        .map(|e| {
                            let mut c = to_card(e, now);
                            c.email = accounts::default_email(&c.id).unwrap_or_default();
                            c.account_key = accounts::DEFAULT_KEY.to_string();
                            c
                        })
                        .collect::<Vec<Card>>()
                })
        });

        let extra_handles: Vec<_> = extras
            .iter()
            .map(|(provider, extra)| {
                let provider = *provider;
                s.spawn(move || {
                    let env = accounts::home_env(provider).map(|k| (k, extra.dir.as_path()));
                    let mut card =
                        match run_core(&["usage", "--json", "-p", provider], env, None).and_then(|j| parse(&j)) {
                            Ok(mut entries) if !entries.is_empty() => to_card(entries.remove(0), now),
                            Ok(_) => error_card(provider, "Veri yok".into(), "No data".into()),
                            Err(e) => error_card(provider, e.clone(), e),
                        };
                    card.email = accounts::extra_email(extra).unwrap_or_default();
                    card.account_key = extra.key.clone();
                    card
                })
            })
            .collect();

        let default_res = default_handle
            .join()
            .unwrap_or_else(|_| Err("Çekirdek iş parçacığı beklenmedik şekilde sonlandı".into()));
        let extra_cards: Vec<Card> = extra_handles.into_iter().filter_map(|h| h.join().ok()).collect();
        (default_res, extra_cards)
    });

    let mut cards = Vec::new();
    let mut default_err = None;
    match default_res {
        Ok(c) => cards.extend(c),
        Err(e) => default_err = Some(e),
    }
    cards.extend(extra_cards);

    if cards.is_empty()
        && let Some(e) = default_err
    {
        return Err(e);
    }

    cards.sort_by_key(|c| (provider_sort_rank(&c.id), c.id.clone()));
    Ok(cards)
}

fn parse(json: &str) -> Result<Vec<Entry>, String> {
    serde_json::from_str(json.trim()).map_err(|e| format!("Çekirdek çıktısı okunamadı: {e}"))
}

#[derive(Debug, Clone)]
pub struct ModelCost {
    pub model: String,
    pub usd: f64,
}

#[derive(Debug, Clone)]
pub struct DailyCost {
    pub day: String,
    pub usd: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ProviderCost {
    pub id: String,
    pub name: String,
    pub total_usd: f64,
    pub sessions: u64,
    pub tokens: u64,
    pub models: Vec<ModelCost>,
}

#[derive(Debug, Clone)]
pub struct CostSummary {
    pub days: u32,
    pub total_usd: f64,
    pub total_tokens: u64,
    pub total_sessions: u64,
    pub providers: Vec<ProviderCost>,
    pub daily: Vec<DailyCost>,
}

#[derive(Deserialize)]
struct CostEntry {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    cost: Option<CostTotal>,
    #[serde(default)]
    sessions_count: Option<u64>,
    #[serde(default)]
    by_model: HashMap<String, f64>,
    #[serde(default, rename = "spendContract")]
    spend_contract: Option<SpendContract>,
}

#[derive(Deserialize)]
struct CostTotal {
    #[serde(default)]
    total_usd: Option<f64>,
}

#[derive(Deserialize)]
struct SpendContract {
    #[serde(default)]
    daily: Vec<SpendDay>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpendDay {
    #[serde(default)]
    day: String,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

pub fn fetch_costs(days: u32) -> Result<CostSummary, String> {
    let days_arg = days.to_string();
    let args = [
        "cost", "-p", "both", "--days", &days_arg, "--format", "json",
    ];
    let json = run_core(&args, None, None)?;
    parse_costs(&json, days)
}

fn parse_costs(json: &str, days: u32) -> Result<CostSummary, String> {
    let entries: Vec<CostEntry> =
        serde_json::from_str(json.trim()).map_err(|e| format!("Maliyet verisi okunamadı: {e}"))?;
    let mut summary = CostSummary {
        days,
        total_usd: 0.0,
        total_tokens: 0,
        total_sessions: 0,
        providers: Vec::new(),
        daily: Vec::new(),
    };
    let mut daily = BTreeMap::<String, (f64, u64)>::new();
    for entry in entries {
        let total_usd = entry
            .cost
            .and_then(|cost| cost.total_usd)
            .unwrap_or_default();
        let sessions = entry.sessions_count.unwrap_or_default();
        let mut tokens = 0;
        if let Some(contract) = entry.spend_contract {
            for point in contract.daily {
                let point_tokens = point.total_tokens.unwrap_or_default();
                tokens += point_tokens;
                if !point.day.is_empty() {
                    let (day_usd, day_tokens) = daily.entry(point.day).or_default();
                    *day_usd += point.cost_usd.unwrap_or_default();
                    *day_tokens += point_tokens;
                }
            }
        }
        summary.total_usd += total_usd;
        summary.total_tokens += tokens;
        summary.total_sessions += sessions;

        if total_usd > 0.0 || sessions > 0 {
            let mut models: Vec<_> = entry
                .by_model
                .into_iter()
                .filter(|(_, usd)| *usd > 0.0001)
                .map(|(model, usd)| ModelCost { model, usd })
                .collect();
            models.sort_by(|a, b| b.usd.total_cmp(&a.usd).then_with(|| a.model.cmp(&b.model)));
            summary.providers.push(ProviderCost {
                name: display_name(&entry.provider),
                id: entry.provider,
                total_usd,
                sessions,
                tokens,
                models,
            });
        }
    }
    summary
        .providers
        .sort_by(|a, b| b.total_usd.total_cmp(&a.total_usd));
    let older_days = daily.len().saturating_sub(14);
    summary.daily = daily
        .into_iter()
        .skip(older_days)
        .map(|(day, (usd, tokens))| DailyCost { day, usd, tokens })
        .collect();
    Ok(summary)
}

pub fn format_tokens(n: u64) -> String {
    let (unit, suffix) = match n {
        1_000_000_000.. => (1_000_000_000.0, "B"),
        1_000_000.. => (1_000_000.0, "M"),
        1_000.. => (1_000.0, "K"),
        _ => return n.to_string(),
    };
    let scaled = format!("{:.2}", n as f64 / unit);
    let scaled = scaled.trim_end_matches('0').trim_end_matches('.');
    format!("{scaled}{suffix}")
}

fn error_card(provider: &str, msg_tr: String, msg_en: String) -> Card {
    Card {
        id: provider.to_string(),
        name: display_name(provider),
        email: String::new(),
        account_key: String::new(),
        plan: String::new(),
        metrics: vec![Metric {
            label_tr: msg_tr,
            label_en: msg_en,
            percent: 0.0,
            detail_tr: "Veri alınamadı".into(),
            detail_en: "Unavailable".into(),
            pace_tr: String::new(),
            pace_en: String::new(),
            pace_level: 0,
            info: true,
            window_minutes: None,
            resets_at: None,
        }],
        failed: true,
    }
}

#[derive(Debug, Clone)]
pub struct CoreProvider {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    #[allow(dead_code, reason = "Preserve the core default alongside the current enabled state")]
    pub default_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderState {
    provider: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    default_enabled: bool,
}

/// Çekirdekteki tüm aktif sağlayıcıların yapılandırma durumu.
pub fn provider_states() -> Result<Vec<CoreProvider>, String> {
    let json = run_core(&["config", "providers", "--format", "json"], None, None)?;
    parse_provider_states(&json)
}

fn parse_provider_states(json: &str) -> Result<Vec<CoreProvider>, String> {
    let all: Vec<ProviderState> =
        serde_json::from_str(json.trim()).map_err(|e| format!("Sağlayıcı listesi okunamadı: {e}"))?;
    Ok(all
        .into_iter()
        .filter(|p| !p.display_name.to_ascii_lowercase().contains("(removed)"))
        .map(|p| CoreProvider {
            display_name: if p.display_name.is_empty() {
                display_name(&p.provider)
            } else {
                p.display_name
            },
            id: p.provider,
            enabled: p.enabled,
            default_enabled: p.default_enabled,
        })
        .collect())
}

fn is_valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    if !is_valid_provider_id(id) {
        return Err(format!("Geçersiz sağlayıcı kimliği: {id}"));
    }
    run_core(&["config", if enabled { "enable" } else { "disable" }, id], None, None).map(|_| ())
}

/// API anahtarını komut satırına yazmadan `--stdin` üzerinden çekirdeğe kaydeder ve sağlayıcıyı açar.
pub fn set_api_key(id: &str, key: &str) -> Result<(), String> {
    if !is_valid_provider_id(id) {
        return Err(format!("Geçersiz sağlayıcı kimliği: {id}"));
    }
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("API anahtarı boş olamaz".into());
    }
    if !uses_api_key(id) {
        return Err(format!("{id} API anahtarı kullanmıyor"));
    }
    let payload = format!("{trimmed}\n");
    run_core(&["config", "set-api-key", "--stdin", id], None, Some(&payload)).map(|_| ())
}

fn run_core(args: &[&str], env: Option<(&str, &std::path::Path)>, stdin_data: Option<&str>) -> Result<String, String> {
    let core = find_core().ok_or_else(|| format!("{CORE_EXE} bulunamadı"))?;
    let mut cmd = Command::new(core);
    if let Some((key, dir)) = env {
        cmd.env(key, dir);
    }
    cmd.args(args)
        .stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Çekirdek başlatılamadı: {e}"))?;

    if let Some(input) = stdin_data
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(input.as_bytes());
    }

    let mut stdout = child.stdout.take().ok_or("stdout alınamadı")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(format!("Çekirdek hata verdi ({status})"));
            }
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > CORE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Çekirdek zaman aşımına uğradı".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("Çekirdek beklenemedi: {e}")),
        }
    }
    reader.join().map_err(|_| "stdout okunamadı".to_string())
}

fn to_card(e: Entry, now: DateTime<Utc>) -> Card {
    if let Some(err) = e.error {
        let (tr, en) = describe_error(e.error_kind.as_deref(), &err);
        return error_card(&e.provider, tr, en);
    }
    let name = display_name(&e.provider);
    let Some(u) = e.usage else {
        return Card {
            id: e.provider,
            name,
            email: String::new(),
            account_key: String::new(),
            plan: String::new(),
            metrics: Vec::new(),
            failed: true,
        };
    };

    let (primary_pace, secondary_pace) = e
        .pace
        .as_ref()
        .map(|pace| (pace.primary.as_ref(), pace.secondary.as_ref()))
        .unwrap_or_default();
    let mut metrics: Vec<Metric> = [
        (
            u.primary_label,
            ("Oturum", "Session"),
            u.primary,
            primary_pace,
        ),
        (
            u.secondary_label,
            ("Haftalık", "Weekly"),
            u.secondary,
            secondary_pace,
        ),
        (
            u.tertiary_label,
            ("Ek pencere", "Extra window"),
            u.tertiary,
            None,
        ),
    ]
    .into_iter()
    .filter_map(|(name, fallback, w, pace)| w.map(|w| (name, fallback, w, pace)))
    .map(|(name, (fb_tr, fb_en), w, pace)| {
        let (label_tr, label_en) = match name {
            Some(n) => (translate_label_tr(&n), n),
            None => (
                window_label_tr(w.window_minutes, fb_tr),
                window_label_en(w.window_minutes, fb_en),
            ),
        };
        metric(label_tr, label_en, &w, now, pace)
    })
    .collect();
    for extra in u.extra_rate_windows {
        let (label_tr, label_en) = match extra.title.or(extra.id) {
            Some(t) if t == "Reset credits" => ("Sıfırlama kredisi".to_string(), t),
            Some(t) => (translate_label_tr(&t), t),
            None => ("Ek".into(), "Extra".into()),
        };
        metrics.push(metric(label_tr, label_en, &extra.window, now, None));
    }

    Card {
        id: e.provider,
        name,
        email: String::new(),
        account_key: String::new(),
        plan: plan_text(u.login_method.as_deref()),
        metrics,
        failed: false,
    }
}

fn metric(
    label_tr: String,
    label_en: String,
    w: &RateWindow,
    now: DateTime<Utc>,
    pace: Option<&PaceInfo>,
) -> Metric {
    if w.is_informational {
        let raw = w.reset_description.as_deref().unwrap_or_default();
        return Metric {
            label_tr,
            label_en,
            percent: 0.0,
            detail_tr: info_text_tr(raw),
            detail_en: info_text_en(raw),
            pace_tr: String::new(),
            pace_en: String::new(),
            pace_level: 0,
            info: true,
            window_minutes: None,
            resets_at: None,
        };
    }
    let (pace_tr, pace_en, pace_level) = pace
        .filter(|p| !p.stage.is_empty())
        .map(format_pace)
        .unwrap_or_default();
    Metric {
        label_tr,
        label_en,
        percent: w.used_percent.clamp(0.0, 100.0) as f32,
        detail_tr: countdown(w, now, Lang::Tr),
        detail_en: countdown(w, now, Lang::En),
        pace_tr,
        pace_en,
        pace_level,
        info: false,
        window_minutes: w.window_minutes,
        resets_at: w.resets_at,
    }
}

fn format_pace(pace: &PaceInfo) -> (String, String, i32) {
    let delta = pace.delta_percent.round() as i32;
    let sign = if delta > 0 { "+" } else { "" };
    let percent = format!("{sign}{delta}%");
    let (tr, en, level) = match pace.stage.to_ascii_lowercase().as_str() {
        "behind" | "farbehind" => ("Rahat", "Calm", 1),
        "ontrack" => ("Tempoda", "On track", 1),
        "ahead" => ("Hızlı", "Fast", 2),
        "farahead" => ("Çok hızlı", "Burning", 3),
        _ => {
            let level = if pace.will_last_to_reset { 1 } else { 2 };
            return (percent.clone(), percent, level);
        }
    };
    (
        format!("{percent} · {tr}"),
        format!("{percent} · {en}"),
        level,
    )
}

pub fn display_name(id: &str) -> String {
    match id {
        "codex" => "Codex".into(),
        "claude" => "Claude".into(),
        "antigravity" => "Antigravity".into(),
        "gemini" => "Gemini".into(),
        "copilot" => "GitHub Copilot".into(),
        "cursor" => "Cursor".into(),
        "windsurf" => "Windsurf".into(),
        "openrouter" => "OpenRouter".into(),
        "deepseek" => "DeepSeek".into(),
        "openaiapi" => "OpenAI API".into(),
        "xai" => "xAI".into(),
        "jetbrains" => "JetBrains".into(),
        "opencode" => "OpenCode".into(),
        "vertexai" => "Vertex AI".into(),
        "huggingface" => "Hugging Face".into(),
        "zai" => "z.ai".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

fn plan_text(login: Option<&str>) -> String {
    match login {
        Some(s) if s.contains('_') && s.contains('(') => s.split('(').next().unwrap_or(s).trim().into(),
        Some(s) => s.into(),
        None => String::new(),
    }
}

fn info_text_tr(raw: &str) -> String {
    if let Some(n) = raw
        .strip_suffix(" reset credits available")
        .or_else(|| raw.strip_suffix(" reset credit available"))
    {
        return format!("{n} kredi");
    }
    raw.into()
}

fn info_text_en(raw: &str) -> String {
    if let Some(n) = raw
        .strip_suffix(" reset credits available")
        .or_else(|| raw.strip_suffix(" reset credit available"))
    {
        return format!("{n} credits");
    }
    raw.into()
}

fn describe_error(kind: Option<&str>, raw: &str) -> (String, String) {
    match kind {
        Some("timeout") => ("Zaman aşımı".into(), "Timed out".into()),
        Some("auth") | Some("unauthorized") => ("Giriş gerekli".into(), "Sign-in required".into()),
        _ => {
            let s: String = raw.chars().take(60).collect();
            let short = if raw.chars().count() > 60 { s + "…" } else { s };
            (short.clone(), short)
        }
    }
}

fn translate_label_tr(raw: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for w in raw.split_whitespace() {
        let lower = w.to_lowercase();
        let tr = match lower.as_str() {
            "weekly" => "haftalık".to_string(),
            "daily" => "günlük".to_string(),
            "monthly" => "aylık".to_string(),
            "session" => "oturum".to_string(),
            _ => {
                let hours = lower.strip_suffix("-hour").or_else(|| lower.strip_suffix('h'));
                match hours.filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_digit())) {
                    Some(h) => format!("{h} saat"),
                    None => w.to_string(),
                }
            }
        };
        words.push(tr);
    }
    words.join(" ")
}

fn window_label_tr(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(10080) => "Haftalık".into(),
        Some(1440) => "Günlük".into(),
        Some(m) if m > 0 && m < 1440 && m % 60 == 0 => format!("{} saat", m / 60),
        Some(m) if m >= 1440 && m % 1440 == 0 => format!("{} gün", m / 1440),
        _ => fallback.into(),
    }
}

fn window_label_en(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(10080) => "Weekly".into(),
        Some(1440) => "Daily".into(),
        Some(m) if m > 0 && m < 1440 && m % 60 == 0 => format!("{}h", m / 60),
        Some(m) if m >= 1440 && m % 1440 == 0 => format!("{}d", m / 1440),
        _ => fallback.into(),
    }
}

fn countdown(w: &RateWindow, now: DateTime<Utc>, lang: Lang) -> String {
    if let Some(at) = w.resets_at {
        let mins = (at - now).num_minutes();
        if mins <= 0 {
            return match lang {
                Lang::Tr => "sıfırlandı".into(),
                Lang::En => "reset".into(),
            };
        }
        let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
        return match lang {
            Lang::Tr => {
                if d > 0 {
                    format!("{d}g {h}sa")
                } else if h > 0 {
                    format!("{h}sa {m}dk")
                } else {
                    format!("{m}dk")
                }
            }
            Lang::En => {
                if d > 0 {
                    format!("{d}d {h}h")
                } else if h > 0 {
                    format!("{h}h {m}m")
                } else {
                    format!("{m}m")
                }
            }
        };
    }
    w.reset_description.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[{"cost":null,"pace":{"primary":{"deltaPercent":-0.01,"stage":"ontrack","willLastToReset":true}},"provider":"codex","source":"oauth","usage":{"extra_rate_windows":[{"id":"reset-credits","title":"Reset credits","window":{"is_informational":true,"reset_description":"3 reset credits available","used_percent":0.0}}],"login_method":"ChatGPT Plus","primary":{"is_informational":false,"reset_description":"5h","resets_at":"2026-11-24T15:48:12Z","used_percent":12.5,"window_minutes":300},"secondary":{"is_informational":false,"resets_at":"2026-11-30T22:46:02Z","used_percent":40.0,"window_minutes":10080},"updated_at":"2026-11-24T10:48:11.219685900Z"}},{"provider":"claude","error":"Network error","errorKind":"timeout"}]"#;

    #[test]
    fn parses_active_provider_states() {
        let providers = parse_provider_states(
            r#"[
                {"provider":"codex","displayName":"Core Codex","enabled":true,"defaultEnabled":true},
                {"provider":"kimik2","displayName":"Kimi K2 (removed)"},
                {"provider":"crossmodel","displayName":"CrossModel (ReMoVeD)"},
                {"provider":"openaiapi","displayName":""},
                {"provider":"ollama"}
            ]"#,
        )
        .unwrap();

        assert_eq!(providers.len(), 3);
        assert_eq!(providers[0].id, "codex");
        assert_eq!(providers[0].display_name, "Core Codex");
        assert!(providers[0].enabled);
        assert!(providers[0].default_enabled);
        assert_eq!(providers[1].id, "openaiapi");
        assert_eq!(providers[1].display_name, "OpenAI API");
        assert!(!providers[1].enabled);
        assert!(!providers[1].default_enabled);
        assert_eq!(providers[2].display_name, "Ollama");
    }

    #[test]
    fn ranks_popular_providers_first() {
        assert_eq!(provider_sort_rank("codex"), 0);
        assert_eq!(provider_sort_rank("vercel"), POPULAR_ORDER.len() - 1);
        assert_eq!(provider_sort_rank("zai"), POPULAR_ORDER.len());
    }

    #[test]
    fn rejects_provider_ids_that_could_be_cli_flags() {
        assert!(is_valid_provider_id("provider-2_beta"));
        assert!(!is_valid_provider_id("--help"));
        assert!(!is_valid_provider_id("-h"));
        assert!(!is_valid_provider_id("codex claude"));
        assert!(!is_valid_provider_id(""));
    }

    #[test]
    fn parses_mixed_entries_bilingual() {
        let now = "2026-11-24T14:00:00Z".parse().unwrap();
        let entries: Vec<Entry> = serde_json::from_str(SAMPLE).unwrap();
        let cards: Vec<Card> = entries.into_iter().map(|e| to_card(e, now)).collect();
        assert_eq!(cards.len(), 2);
        let codex = &cards[0];
        assert_eq!(codex.name, "Codex");
        assert!(codex.has_5h_window());
        assert_eq!(codex.primary_5h_percent(), Some(12.5));
        assert_eq!(codex.metrics[0].label(Lang::Tr), "5 saat");
        assert_eq!(codex.metrics[0].label(Lang::En), "5h");
        assert_eq!(codex.metrics[0].detail(Lang::Tr), "1sa 48dk");
        assert_eq!(codex.metrics[0].detail(Lang::En), "1h 48m");
        assert_eq!(codex.metrics[0].pace(Lang::Tr), "0% · Tempoda");
        assert_eq!(codex.metrics[0].pace(Lang::En), "0% · On track");
        assert_eq!(codex.metrics[0].pace_level, 1);
        assert_eq!(codex.metrics[1].pace_level, 0);
        assert_eq!(codex.metrics[2].detail(Lang::Tr), "3 kredi");
        assert_eq!(codex.metrics[2].detail(Lang::En), "3 credits");
        assert_eq!(codex.metrics[2].pace_level, 0);
        assert!(cards[1].failed);
        assert!(!cards[1].has_5h_window());
        assert_eq!(cards[1].metrics[0].label(Lang::En), "Timed out");
        assert_eq!(cards[1].metrics[0].pace_level, 0);
    }

    #[test]
    fn attaches_pace_to_primary_and_secondary_only() {
        let entry = serde_json::from_str(
            r#"{
                "provider": "codex",
                "usage": {
                    "primary": {},
                    "secondary": {},
                    "tertiary": {},
                    "extra_rate_windows": [{"window": {}}]
                },
                "pace": {
                    "primary": {"deltaPercent": -7.06, "stage": "BeHiNd", "willLastToReset": true},
                    "secondary": {"deltaPercent": 33.64, "stage": "farahead", "willLastToReset": false}
                }
            }"#,
        )
        .unwrap();
        let card = to_card(entry, Utc::now());

        assert_eq!(card.metrics[0].pace(Lang::Tr), "-7% · Rahat");
        assert_eq!(card.metrics[0].pace(Lang::En), "-7% · Calm");
        assert_eq!(card.metrics[0].pace_level, 1);
        assert_eq!(card.metrics[1].pace(Lang::Tr), "+34% · Çok hızlı");
        assert_eq!(card.metrics[1].pace(Lang::En), "+34% · Burning");
        assert_eq!(card.metrics[1].pace_level, 3);
        for row in &card.metrics[2..] {
            assert_eq!(row.pace(Lang::Tr), "");
            assert_eq!(row.pace(Lang::En), "");
            assert_eq!(row.pace_level, 0);
        }
    }

    #[test]
    fn formats_other_pace_stages_and_skips_empty_or_informational_pace() {
        let now = Utc::now();
        for (stage, will_last, delta, tr, en, level) in [
            ("farbehind", false, -8.5, "-9% · Rahat", "-9% · Calm", 1),
            ("ahead", true, 5.5, "+6% · Hızlı", "+6% · Fast", 2),
            ("unknown", true, 1.0, "+1%", "+1%", 1),
            ("unknown", false, 1.0, "+1%", "+1%", 2),
            ("", false, 1.0, "", "", 0),
        ] {
            let window = serde_json::from_str("{}").unwrap();
            let pace = PaceInfo {
                stage: stage.into(),
                will_last_to_reset: will_last,
                delta_percent: delta,
            };
            let row = metric(String::new(), String::new(), &window, now, Some(&pace));
            assert_eq!(
                (row.pace(Lang::Tr), row.pace(Lang::En), row.pace_level),
                (tr, en, level)
            );
        }

        let entry = serde_json::from_str(
            r#"{
                "provider": "claude",
                "usage": {"secondary": {"is_informational": true}},
                "pace": {"secondary": {"stage": "ahead"}}
            }"#,
        )
        .unwrap();
        let card = to_card(entry, now);
        let row = &card.metrics[0];
        assert_eq!(
            (row.pace(Lang::Tr), row.pace(Lang::En), row.pace_level),
            ("", "", 0)
        );
    }

    #[test]
    fn parses_costs_and_merges_daily_spend() {
        let summary = parse_costs(
            r#"[
                {
                    "provider": "codex",
                    "cost": {"total_usd": 3.5},
                    "sessions_count": 2,
                    "by_model": {"small": 0.5, "large": 3.0, "zero": 0, "tiny": 0.0001},
                    "spendContract": {"daily": [
                        {"day": "2026-11-24", "costUsd": 2.5, "totalTokens": 200},
                        {"day": "2026-11-22", "costUsd": 1.0, "totalTokens": 100}
                    ]}
                },
                {
                    "provider": "claude",
                    "cost": {"total_usd": 5.0},
                    "sessions_count": 3,
                    "spendContract": {"daily": [
                        {"day": "2026-11-24", "costUsd": 5.0, "totalTokens": 400},
                        {"day": "2026-11-23", "costUsd": null, "totalTokens": null}
                    ]}
                },
                {"provider": "pi", "sessions_count": 1, "cost": {}, "spendContract": null},
                {"provider": "unsupported"},
                {}
            ]"#,
            7,
        )
        .unwrap();

        assert_eq!(summary.days, 7);
        assert_eq!(summary.total_usd, 8.5);
        assert_eq!(summary.total_tokens, 700);
        assert_eq!(summary.total_sessions, 6);
        let providers: Vec<_> = summary
            .providers
            .iter()
            .map(|p| {
                (
                    p.id.as_str(),
                    p.name.as_str(),
                    p.total_usd,
                    p.sessions,
                    p.tokens,
                )
            })
            .collect();
        assert_eq!(
            providers,
            [
                ("claude", "Claude", 5.0, 3, 400),
                ("codex", "Codex", 3.5, 2, 300),
                ("pi", "Pi", 0.0, 1, 0),
            ]
        );
        let models: Vec<_> = summary.providers[1]
            .models
            .iter()
            .map(|m| (m.model.as_str(), m.usd))
            .collect();
        assert_eq!(models, [("large", 3.0), ("small", 0.5)]);
        let daily: Vec<_> = summary
            .daily
            .iter()
            .map(|d| (d.day.as_str(), d.usd, d.tokens))
            .collect();
        assert_eq!(
            daily,
            [
                ("2026-11-22", 1.0, 100),
                ("2026-11-23", 0.0, 0),
                ("2026-11-24", 7.5, 600),
            ]
        );
    }

    #[test]
    fn retains_the_latest_fourteen_days_and_counts_all_tokens() {
        let daily: Vec<_> = (1..=15)
            .rev()
            .map(|day| serde_json::json!({"day": format!("2026-11-{day:02}"), "totalTokens": 10}))
            .collect();
        let json = serde_json::json!([{"provider": "codex", "spendContract": {"daily": daily}}]);
        let summary = parse_costs(&json.to_string(), 30).unwrap();

        assert_eq!(summary.daily.len(), 14);
        assert_eq!(summary.daily[0].day, "2026-11-02");
        assert_eq!(summary.daily[13].day, "2026-11-15");
        assert_eq!(summary.total_tokens, 150);
    }

    #[test]
    fn formats_tokens_compactly() {
        for (count, expected) in [
            (0, "0"),
            (950, "950"),
            (1_000, "1K"),
            (45_200, "45.2K"),
            (1_000_000, "1M"),
            (176_400_000, "176.4M"),
            (1_250_000_000, "1.25B"),
        ] {
            assert_eq!(format_tokens(count), expected);
        }
    }

    #[test]
    fn weekly_and_informational_metrics_do_not_count_as_five_hour_windows() {
        let entry = serde_json::from_str(
            r#"{
                "provider": "claude",
                "usage": {
                    "primary": {"used_percent": 12.5, "window_minutes": 10080},
                    "secondary": {"is_informational": true, "window_minutes": 300}
                }
            }"#,
        )
        .unwrap();
        let card = to_card(entry, Utc::now());

        assert_eq!(card.metrics[0].window_minutes, Some(10080));
        assert_eq!(card.metrics[1].window_minutes, None);
        assert!(!card.has_5h_window());
        assert_eq!(card.primary_5h_percent(), None);
    }

    #[test]
    fn translates_core_labels() {
        assert_eq!(translate_label_tr("Gemini 5h"), "Gemini 5 saat");
        assert_eq!(translate_label_tr("Gemini Weekly"), "Gemini haftalık");
        assert_eq!(translate_label_tr("Claude/GPT 5-hour"), "Claude/GPT 5 saat");
        assert_eq!(translate_label_tr("Claude/GPT weekly"), "Claude/GPT haftalık");
    }

    #[test]
    fn hides_internal_plan_names() {
        assert_eq!(plan_text(Some("Claude (default_claude_ai)")), "Claude");
        assert_eq!(plan_text(Some("ChatGPT Plus")), "ChatGPT Plus");
    }
}
